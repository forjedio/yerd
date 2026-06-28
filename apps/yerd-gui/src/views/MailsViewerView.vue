<script setup lang="ts">
import { Inbox, Trash2 } from "lucide-vue-next";
import { computed, onUnmounted, ref, watch } from "vue";

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
} from "@/ipc/client";
import type { MailDetail, MailSummary } from "@/ipc/types";

// The rendered HTML email is sandboxed: no scripts, no same-origin. The child CSP
// keeps `default-src 'none'` (so nothing executes), but allows images over
// data:/http/https so emails render with their inline AND remote images (logos
// etc.). Note: like any mail client, this means remote images can load when you
// open a message.
const CHILD_CSP =
  "default-src 'none'; img-src data: http: https:; style-src 'unsafe-inline'";

const toast = useToast();

// Live list of captured mail (newest first).
const { data: mails, refresh } = usePoll<MailSummary[]>(listMails, 4000);

onUnmounted(registerViewActions({ refresh: () => void refresh() }));

const selectedId = ref<string | null>(null);
const detail = ref<MailDetail | null>(null);
const loadingDetail = ref(false);
// Monotonic guard so an out-of-order getMail() response from a superseded
// select() can't overwrite the body of a newer selection.
let selectSeq = 0;
const clearOpen = ref(false);
const clearing = ref(false);
// Filter the list to a single application (or "" = all). Laravel sends the app
// name as the From display name (MAIL_FROM_NAME = config('app.name')); we group
// by that, falling back to the From email when there's no display name.
const selectedApp = ref<string>("");

const list = computed<MailSummary[]>(() => mails.value ?? []);

/** The "application" an email belongs to: its From display name, or - when the
 *  From has no name - the bare email address. */
function applicationOf(from: string): string {
  const named = from.match(/^\s*(.*?)\s*<([^>]+)>\s*$/);
  if (named) {
    const name = named[1].replace(/^"|"$/g, "").trim();
    return name || named[2].trim();
  }
  return from.trim();
}

// Distinct applications present, for the filter dropdown.
const applications = computed<string[]>(() => {
  const set = new Set<string>();
  for (const m of list.value) set.add(applicationOf(m.from));
  return [...set].sort((a, b) => a.localeCompare(b));
});

const appOptions = computed(() => [
  { value: "", label: `All applications (${list.value.length})` },
  ...applications.value.map((a) => ({ value: a, label: a })),
]);

// The list narrowed to the selected application.
const filteredList = computed<MailSummary[]>(() =>
  selectedApp.value
    ? list.value.filter((m) => applicationOf(m.from) === selectedApp.value)
    : list.value,
);

// If the selected application disappears (e.g. its mail was cleared), fall back
// to showing everything.
watch(applications, (apps) => {
  if (selectedApp.value && !apps.includes(selectedApp.value)) {
    selectedApp.value = "";
  }
});

// Auto-select the first visible message; keep the selection valid as the
// (filtered) list changes.
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

/**
 * Open a message in the reading pane. `fromUser` distinguishes a genuine row click
 * (which marks the mail read) from the watcher's auto-selection (which must not).
 * A monotonic `selectSeq` suppresses a superseded in-flight `getMail`, so an
 * out-of-order response can't overwrite a newer selection.
 */
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

/**
 * Mark one email read on a genuine open: persist via the daemon, then replace the
 * list array (usePoll's data is a shallowRef, so an in-place mutation wouldn't be
 * reactive) so the unread styling/badges clear immediately. Best-effort: a failure
 * just leaves it unread and the next poll reconciles. Only acts when the daemon
 * explicitly reported the mail unread (`read === false`); an older daemon omits
 * `read` (and the `MarkMailsRead` request), so we must not call it there.
 */
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

// The delete button is scoped to the current filter: with no application
// selected it clears everything; with one selected it deletes only that
// application's currently-shown emails.
const deleteScopeLabel = computed(() =>
  selectedApp.value ? `all ${filteredList.value.length} email(s) from “${selectedApp.value}”` : "every captured email",
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

function frameSrcdoc(html: string): string {
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${CHILD_CSP}"></head><body>${html}</body></html>`;
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
          <iframe
            v-if="detail.html_body"
            :srcdoc="frameSrcdoc(detail.html_body)"
            sandbox=""
            class="min-h-0 flex-1 bg-white"
            title="Email body"
          />
          <pre
            v-else
            class="min-h-0 flex-1 overflow-auto whitespace-pre-wrap p-5 text-sm"
          >{{ detail.text_body || "(empty message)" }}</pre>
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
