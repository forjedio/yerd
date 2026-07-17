<script setup lang="ts">
import { FileDown, Inbox, Paperclip, Trash2 } from "lucide-vue-next";
import { computed, nextTick, onUnmounted, ref, watch } from "vue";

import TitleBar from "@/components/TitleBar.vue";
import Button from "@/components/ui/Button.vue";
import Modal from "@/components/ui/Modal.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { log } from "@/lib/log";
import { usePoll } from "@/composables/usePoll";
import { useToast } from "@/composables/useToast";
import {
  clearMails,
  deleteMails,
  getMail,
  IpcError,
  listMails,
  markMailsRead,
  openInEditor,
  openInBrowser,
  saveMailAttachment,
} from "@/ipc/client";
import { linkifyText, prepareHtmlBody, resolveFrameLink } from "@/lib/mailLinks";
import { humaniseBytes } from "@/lib/utils";
import type { MailAttachment, MailDetail, MailSummary } from "@/ipc/types";

// HTML emails are rendered in a Shadow DOM (style isolation, no email scripts).
// WKWebView never delivered click events from a sandboxed srcdoc iframe to
// parent-attached listeners, so the host Shadow root owns the click path.

const toast = useToast();

const { data: mails, refresh } = usePoll<MailSummary[]>(listMails, 4000);

const unregisterViewActions = registerViewActions({ refresh: () => void refresh() });
onUnmounted(() => {
  unregisterViewActions();
});

const selectedId = ref<string | null>(null);
const detail = ref<MailDetail | null>(null);
const loadingDetail = ref(false);
let selectSeq = 0;
const clearOpen = ref(false);
const clearing = ref(false);
const selectedApp = ref<string>("");

const htmlHost = ref<HTMLElement | null>(null);
let lastOpenedLink = "";
let lastOpenedAt = 0;

const list = computed<MailSummary[]>(() => mails.value ?? []);

function applicationOf(from: string): string {
  const named = from.match(/^\s*(.*?)\s*<([^>]+)>\s*$/);
  if (named) {
    const name = named[1].replace(/^"|"$/g, "").trim();
    return name || named[2].trim();
  }
  return from.trim();
}

const applications = computed<string[]>(() => {
  const set = new Set<string>();
  for (const m of list.value) set.add(applicationOf(m.from));
  return [...set].sort((a, b) => a.localeCompare(b));
});

const appOptions = computed(() => [
  { value: "", label: `All applications (${list.value.length})` },
  ...applications.value.map((a) => ({ value: a, label: a })),
]);

const filteredList = computed<MailSummary[]>(() =>
  selectedApp.value
    ? list.value.filter((m) => applicationOf(m.from) === selectedApp.value)
    : list.value,
);

watch(applications, (apps) => {
  if (selectedApp.value && !apps.includes(selectedApp.value)) {
    selectedApp.value = "";
  }
});

watch(
  filteredList,
  (items) => {
    if (items.length === 0) {
      selectedId.value = null;
      detail.value = null;
      return;
    }
    if (!selectedId.value || !items.some((m) => m.id === selectedId.value)) {
      void select(items[0].id, false);
    }
  },
  { immediate: true },
);

async function select(id: string, fromUser: boolean): Promise<void> {
  const seq = ++selectSeq;
  selectedId.value = id;
  loadingDetail.value = true;
  try {
    const mail = await getMail(id);
    if (seq !== selectSeq) return;
    detail.value = mail;
    if (fromUser) void markRead(id);
  } catch (e) {
    if (seq !== selectSeq) return;
    toast.error("Couldn't open the email", (e as IpcError).message);
    detail.value = null;
  } finally {
    if (seq === selectSeq) loadingDetail.value = false;
  }
}

async function markRead(id: string): Promise<void> {
  const current = mails.value?.find((m) => m.id === id);
  if (!current || current.read !== false) return;
  try {
    await markMailsRead([id]);
    if (mails.value) {
      mails.value = mails.value.map((m) =>
        m.id === id ? { ...m, read: true } : m,
      );
    }
  } catch (e) {
    log.warn(`mark mail read failed: ${(e as IpcError).message}`);
  }
}

const deleteScopeLabel = computed(() =>
  selectedApp.value
    ? `all ${filteredList.value.length} email(s) from "${selectedApp.value}"`
    : "every captured email",
);

async function confirmDelete(close: () => void): Promise<void> {
  clearing.value = true;
  try {
    if (selectedApp.value) {
      await deleteMails(filteredList.value.map((m) => m.id));
    } else {
      await clearMails();
    }
    selectedId.value = null;
    detail.value = null;
    close();
    await refresh();
    toast.success(selectedApp.value ? "Emails deleted" : "Mailbox cleared");
  } catch (e) {
    toast.error("Couldn't delete emails", (e as IpcError).message);
  } finally {
    clearing.value = false;
  }
}

function onHtmlLinkClick(ev: Event): void {
  const action = resolveFrameLink(ev.target);
  if (!action || action.kind === "scroll") return;
  ev.preventDefault();
  ev.stopPropagation();
  if (action.kind !== "open") return;
  const now = Date.now();
  if (action.url === lastOpenedLink && now - lastOpenedAt < 750) return;
  lastOpenedLink = action.url;
  lastOpenedAt = now;
  void openInBrowser(action.url).catch((e) => {
    toast.error("Couldn't open link", (e as IpcError).message || action.url);
  });
}

function renderHtmlBody(html: string): void {
  const host = htmlHost.value;
  if (!host) return;
  const root = host.shadowRoot ?? host.attachShadow({ mode: "open" });
  root.innerHTML = `<style>
:host { display: block; }
.yerd-mail-body {
  padding: 1.25rem;
  box-sizing: border-box;
  color: #111;
  font: 14px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
}
.yerd-mail-body img, .yerd-mail-body table { max-width: 100%; }
.yerd-mail-body a { cursor: pointer; }
</style><div class="yerd-mail-body">${prepareHtmlBody(html)}</div>`;
  root.addEventListener("click", onHtmlLinkClick, true);
}

function clearHtmlBody(): void {
  const host = htmlHost.value;
  if (!host?.shadowRoot) return;
  host.shadowRoot.innerHTML = "";
}

watch(
  [() => detail.value?.html_body, loadingDetail, htmlHost],
  ([html, loading]) => {
    if (loading) return;
    if (!html) {
      clearHtmlBody();
      return;
    }
    void nextTick(() => renderHtmlBody(html));
  },
);

function handleTextLinkClick(ev: MouseEvent): void {
  const action = resolveFrameLink(ev.target);
  if (!action || action.kind !== "open") return;
  ev.preventDefault();
  void openInBrowser(action.url).catch((e) =>
    toast.error("Couldn't open link", (e as IpcError).message || action.url),
  );
}

const attachments = computed<MailAttachment[]>(
  () => detail.value?.attachments ?? [],
);

async function openAttachment(att: MailAttachment): Promise<void> {
  try {
    const path = await saveMailAttachment(att.filename, att.data);
    await openInEditor(path);
  } catch (e) {
    toast.error("Couldn't open attachment", (e as IpcError).message || att.filename);
  }
}

function mimeLabel(contentType: string): string {
  const sub = contentType.split("/")[1] ?? contentType;
  return sub.replace(/^x-/, "").toUpperCase();
}

function formatDate(epoch: number): string {
  if (!epoch) return "-";
  return new Date(epoch * 1000).toLocaleString();
}
</script>

<template>
  <div class="flex h-screen flex-col bg-background">
    <!-- Custom dark titlebar (matches the main window; the window is
         decorationless), with the clear-all action on the right. -->
    <TitleBar title="Mails">
      <template #actions>
        <!-- Filter to one application (From display name) / from-email. -->
        <Select
          v-model="selectedApp"
          :options="appOptions"
          :disabled="list.length === 0"
          aria-label="Filter by application"
          class="!h-6 max-w-44 text-xs"
        />
        <!-- Plain button (not the icon-variant Button) so it sits cleanly in the
             32px titlebar without overflowing it. -->
        <button
          type="button"
          class="inline-flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-40"
          :disabled="filteredList.length === 0"
          :aria-label="selectedApp ? 'Delete emails for this application' : 'Delete all emails'"
          @click="clearOpen = true"
        >
          <Trash2 class="size-3.5" />
        </button>
      </template>
    </TitleBar>

    <div class="flex min-h-0 flex-1">
      <!-- List pane -->
      <aside class="w-72 shrink-0 overflow-y-auto border-r">
        <div
          v-if="filteredList.length === 0"
          class="flex h-full flex-col items-center justify-center gap-2 p-6 text-center text-muted-foreground"
        >
          <Inbox class="size-8" />
          <p class="text-sm">
            {{ list.length === 0 ? "No captured emails yet" : "No emails for this application" }}
          </p>
        </div>
        <ul v-else class="divide-y">
          <li
            v-for="m in filteredList"
            :key="m.id"
            class="cursor-pointer px-3 py-2.5 transition-colors hover:bg-accent/60"
            :class="m.id === selectedId ? 'bg-accent' : ''"
            @click="select(m.id, true)"
          >
            <div class="flex items-center justify-between gap-2">
              <span class="flex min-w-0 items-center gap-1.5">
                <span
                  v-if="m.read === false"
                  class="size-2 shrink-0 rounded-full bg-brand"
                  aria-label="Unread"
                />
                <span class="truncate text-xs font-medium">{{ applicationOf(m.from) }}</span>
              </span>
              <span class="shrink-0 text-[10px] text-muted-foreground">
                {{ formatDate(m.date_epoch) }}
              </span>
            </div>
            <p class="mt-0.5 truncate text-sm" :class="m.read === false ? 'font-semibold' : ''">
              {{ m.subject || "(no subject)" }}
            </p>
          </li>
        </ul>
      </aside>

      <!-- Body pane -->
      <main class="flex min-w-0 flex-1 flex-col">
        <div
          v-if="loadingDetail"
          class="flex flex-1 items-center justify-center"
        >
          <Spinner class="size-6" />
        </div>
        <template v-else-if="detail">
          <!-- Message header strip -->
          <div class="shrink-0 border-b px-5 py-3">
            <h2 class="text-base font-semibold">
              {{ detail.subject || "(no subject)" }}
            </h2>
            <p class="mt-1 text-xs text-muted-foreground">
              <strong>From:</strong> {{ detail.from }}
            </p>
            <p class="text-xs text-muted-foreground">
              <strong>To:</strong> {{ detail.to.join(", ") }}
            </p>
            <p class="text-xs text-muted-foreground">
              {{ formatDate(detail.date_epoch) }}
            </p>
          </div>

          <!-- HTML body in Shadow DOM (WKWebView iframe clicks never fired) -->
          <div
            v-if="detail.html_body"
            ref="htmlHost"
            class="min-h-0 flex-1 overflow-y-auto bg-white"
            role="article"
            aria-label="Email body"
          />

          <!-- Plain-text body with linkified URLs -->
          <!-- eslint-disable-next-line vue/no-v-html -->
          <pre
            v-else
            class="min-h-0 flex-1 overflow-auto whitespace-pre-wrap p-5 text-sm"
            @click="handleTextLinkClick"
            v-html="linkifyText(detail.text_body ?? '(empty message)')"
          />

          <!-- Attachment bar - shown only when the message has attachments -->
          <div
            v-if="attachments.length > 0"
            class="shrink-0 border-t bg-muted/20 px-5 py-2"
          >
            <div class="mb-1.5 flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
              <Paperclip class="size-3.5" />
              {{ attachments.length === 1 ? "1 attachment" : `${attachments.length} attachments` }}
            </div>
            <div class="flex flex-wrap gap-2">
              <button
                v-for="att in attachments"
                :key="att.filename"
                type="button"
                class="flex items-center gap-2 rounded-md border bg-background px-3 py-1.5 text-left text-xs transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                :title="`Open ${att.filename} (${humaniseBytes(att.size)})`"
                @click="void openAttachment(att)"
              >
                <FileDown class="size-3.5 shrink-0 text-muted-foreground" />
                <span class="flex flex-col leading-tight">
                  <span class="max-w-[14rem] truncate font-medium">{{ att.filename }}</span>
                  <span class="text-[10px] text-muted-foreground">
                    {{ mimeLabel(att.content_type) }} · {{ humaniseBytes(att.size) }}
                  </span>
                </span>
              </button>
            </div>
          </div>
        </template>
        <div
          v-else
          class="flex flex-1 items-center justify-center text-sm text-muted-foreground"
        >
          Select an email to read it
        </div>
      </main>

      <!-- Headers pane -->
      <aside
        v-if="detail"
        class="w-80 shrink-0 overflow-y-auto border-l bg-muted/30 p-4"
      >
        <h3 class="mb-2 text-xs font-semibold uppercase text-muted-foreground">
          Header
        </h3>
        <dl class="space-y-1.5">
          <div v-for="(h, i) in detail.headers" :key="i" class="text-xs">
            <dt class="font-medium">{{ h.name }}</dt>
            <dd class="break-words text-muted-foreground">{{ h.value }}</dd>
          </div>
        </dl>
      </aside>
    </div>

    <Modal
      v-model:open="clearOpen"
      :title="selectedApp ? 'Delete these emails?' : 'Clear all mails?'"
    >
      <p class="text-sm text-muted-foreground">
        This permanently deletes {{ deleteScopeLabel }}. This cannot be undone.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button
          variant="destructive"
          :disabled="clearing"
          @click="confirmDelete(close)"
        >
          <Spinner v-if="clearing" class="size-4" />
          {{ selectedApp ? "Delete" : "Delete all" }}
        </Button>
      </template>
    </Modal>
  </div>
</template>
