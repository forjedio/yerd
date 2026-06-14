<script setup lang="ts">
import { Inbox, Trash2 } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { usePoll } from "@/composables/usePoll";
import { useToast } from "@/composables/useToast";
import { clearMails, getMail, IpcError, listMails } from "@/ipc/client";
import type { MailDetail, MailSummary } from "@/ipc/types";

// The rendered HTML email is sandboxed: no scripts, no same-origin, and a strict
// child CSP injected into the srcdoc so only inline styles + data: images load.
const CHILD_CSP = "default-src 'none'; img-src data:; style-src 'unsafe-inline'";

const toast = useToast();

// Live list of captured mail (newest first).
const { data: mails, refresh } = usePoll<MailSummary[]>(listMails, 4000);

const selectedId = ref<string | null>(null);
const detail = ref<MailDetail | null>(null);
const loadingDetail = ref(false);
const clearOpen = ref(false);
const clearing = ref(false);

const list = computed<MailSummary[]>(() => mails.value ?? []);

// Auto-select the first message; keep the selection valid as the list changes.
watch(
  list,
  (items) => {
    if (items.length === 0) {
      selectedId.value = null;
      detail.value = null;
      return;
    }
    if (!selectedId.value || !items.some((m) => m.id === selectedId.value)) {
      void select(items[0].id);
    }
  },
  { immediate: true },
);

async function select(id: string): Promise<void> {
  selectedId.value = id;
  loadingDetail.value = true;
  try {
    detail.value = await getMail(id);
  } catch (e) {
    toast.error("Couldn't open the email", (e as IpcError).message);
    detail.value = null;
  } finally {
    loadingDetail.value = false;
  }
}

async function confirmClear(close: () => void): Promise<void> {
  clearing.value = true;
  try {
    await clearMails();
    selectedId.value = null;
    detail.value = null;
    close();
    await refresh();
    toast.success("Mailbox cleared");
  } catch (e) {
    toast.error("Couldn't clear the mailbox", (e as IpcError).message);
  } finally {
    clearing.value = false;
  }
}

function frameSrcdoc(html: string): string {
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${CHILD_CSP}"></head><body>${html}</body></html>`;
}

function formatDate(epoch: number): string {
  if (!epoch) return "—";
  return new Date(epoch * 1000).toLocaleString();
}
</script>

<template>
  <div class="flex h-screen flex-col bg-background">
    <!-- Toolbar -->
    <header
      class="flex shrink-0 items-center justify-between border-b px-4 py-2.5"
    >
      <h1 class="text-sm font-semibold">Mails</h1>
      <Button
        variant="ghost"
        size="icon"
        :disabled="list.length === 0"
        aria-label="Clear all mails"
        @click="clearOpen = true"
      >
        <Trash2 class="size-4" />
      </Button>
    </header>

    <div class="flex min-h-0 flex-1">
      <!-- List pane -->
      <aside class="w-72 shrink-0 overflow-y-auto border-r">
        <div
          v-if="list.length === 0"
          class="flex h-full flex-col items-center justify-center gap-2 p-6 text-center text-muted-foreground"
        >
          <Inbox class="size-8" />
          <p class="text-sm">No captured emails yet</p>
        </div>
        <ul v-else class="divide-y">
          <li
            v-for="m in list"
            :key="m.id"
            class="cursor-pointer px-3 py-2.5 transition-colors hover:bg-accent/60"
            :class="m.id === selectedId ? 'bg-accent' : ''"
            @click="select(m.id)"
          >
            <div class="flex items-center justify-between gap-2">
              <span class="truncate text-xs text-muted-foreground">{{ m.from }}</span>
              <span class="shrink-0 text-[10px] text-muted-foreground">
                {{ formatDate(m.date_epoch) }}
              </span>
            </div>
            <p class="mt-0.5 truncate text-sm font-medium">
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

    <Modal v-model:open="clearOpen" title="Clear all mails?">
      <p class="text-sm text-muted-foreground">
        This permanently deletes every captured email. This cannot be undone.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button
          variant="destructive"
          :disabled="clearing"
          @click="confirmClear(close)"
        >
          <Spinner v-if="clearing" class="size-4" /> Delete all
        </Button>
      </template>
    </Modal>
  </div>
</template>
