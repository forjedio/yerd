<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";

import MailInbox from "@/components/mail/MailInbox.vue";
import MailReader from "@/components/mail/MailReader.vue";
import TitleBar from "@/components/TitleBar.vue";
import { useMailbox } from "@/composables/useMailbox";
import { status } from "@/ipc/client";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";

/**
 * The separate Mails window (opened from the sidebar badge, the tray, ⇧⌘M and
 * the Mail screen's pop-out button): the same inbox and reader as the Mail
 * screen, always side by side. The window has no status poller, so the inbox
 * shows no capture state; it reads the site TLD once so local images load.
 */
const box = useMailbox({ autoSelect: () => true });
const { app, busy, detail, failedId, filtered, list, loadingDetail, now, selectedId } = box;

const tld = ref<string>();

async function loadTld(): Promise<void> {
  try {
    tld.value = (await status()).tld;
  } catch {
    tld.value = undefined;
  }
}

onMounted(() => void loadTld());
onUnmounted(registerViewActions({ refresh: () => void box.refresh() }));
</script>

<template>
  <div class="flex h-screen flex-col bg-background">
    <TitleBar title="Mails" />

    <div class="flex min-h-0 flex-1">
      <div class="flex w-[360px] shrink-0 flex-col border-r pl-4 pt-2.5">
        <MailInbox
          v-model:app="app"
          class="flex-1"
          layout="list"
          :mails="filtered"
          :total="list.length"
          :unread="box.unread.value"
          :apps="box.apps.value"
          :selected="selectedId"
          :now="now"
          :loading="box.loading.value"
          :error="box.error.value"
          :clearing="busy === 'clear'"
          @select="(id: string) => box.select(id, true)"
          @clear-all="box.clearAll"
          @retry="box.refresh"
        />
      </div>

      <div class="min-w-0 flex-1">
        <MailReader
          :detail="detail"
          :failed-id="failedId"
          :tld="tld"
          :loading="loadingDetail"
          :now="now"
          :deleting="busy === 'delete'"
          @delete="box.deleteMail"
        />
      </div>
    </div>
  </div>
</template>
