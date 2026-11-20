<script lang="ts">
import { ref } from "vue";

/** The daemon boot in which capture was last toggled. Module scope, so the
 *  "Restart Yerd to apply" pill survives leaving and reopening the screen. */
const pendingBoot = ref<number | null>(null);
</script>

<script setup lang="ts">
import { AppWindow, SlidersHorizontal, TriangleAlert } from "lucide-vue-next";
import { computed, onUnmounted, watch } from "vue";
import { useRouter } from "vue-router";

import MailInbox from "@/components/mail/MailInbox.vue";
import MailReader from "@/components/mail/MailReader.vue";
import MailSetupDialog from "@/components/mail/MailSetupDialog.vue";
import PageHeader from "@/components/PageHeader.vue";
import Button from "@/components/ui/Button.vue";
import InspectorFrame from "@/components/ui/InspectorFrame.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useMailbox } from "@/composables/useMailbox";
import { useToast } from "@/composables/useToast";
import { useWideLayout } from "@/composables/useWideLayout";
import { IpcError, setMailEnabled, showMailsWindow } from "@/ipc/client";
import { copyWithToast, toastOnError } from "@/lib/toastActions";
import { captureStatus, laravelEnv } from "@/lib/mail";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { CHIP_TONE, cn } from "@/lib/utils";

const toast = useToast();
const router = useRouter();
const { report, refresh } = useDaemon();
const wide = useWideLayout();
const box = useMailbox({ autoSelect: () => wide.value });
const { app, busy, detail, filtered, list, loadingDetail, now, selectedId } = box;

onUnmounted(
  registerViewActions({
    refresh: () => {
      void refresh();
      void box.refresh();
    },
  }),
);

/** Live mail status from the shared 4s status poll (no extra loop). */
const mail = computed(() => report.value?.mail ?? null);

const capture = computed(() => {
  const boot = report.value?.boot_id ?? null;
  return captureStatus(mail.value, pendingBoot.value != null && pendingBoot.value === boot);
});

const pillClass = computed(() =>
  cn(
    "inline-flex h-7 shrink-0 items-center gap-2 rounded-full px-3 text-xs font-semibold",
    capture.value.tone === "ok" && CHIP_TONE.success,
    capture.value.tone === "warn" && CHIP_TONE.warning,
    (capture.value.tone === "off" || capture.value.tone === "pending") && "border bg-muted text-muted-foreground",
  ),
);

const setupOpen = ref(false);
const toggling = ref(false);

async function toggleCapture(next: boolean): Promise<void> {
  if (toggling.value) return;
  toggling.value = true;
  try {
    await setMailEnabled(next);
    pendingBoot.value = report.value?.boot_id ?? null;
    toast.success(next ? "Mail capture turned on" : "Mail capture turned off", "Applies when Yerd restarts");
    await refresh();
  } catch (e) {
    toast.error("Couldn't update mail capture", (e as IpcError).message);
  } finally {
    toggling.value = false;
  }
}

const fromName = ref("");
const fromAddress = ref("");
const envText = computed(() =>
  laravelEnv({ port: mail.value?.port ?? 2525, fromName: fromName.value, fromAddress: fromAddress.value }),
);

async function copyEnv(): Promise<void> {
  await copyWithToast(envText.value, {
    title: "Copied to clipboard",
    detail: "Paste these into your Laravel .env",
  });
}

function changePort(): void {
  setupOpen.value = false;
  void router.push("/general");
}

async function openWindow(): Promise<void> {
  await toastOnError("Couldn't open the mail viewer", () => showMailsWindow());
}

const drawerOpen = ref(false);

watch(wide, (isWide) => {
  if (isWide) drawerOpen.value = false;
});

watch(selectedId, (id) => {
  if (!id) drawerOpen.value = false;
});

function onSelect(id: string): void {
  void box.select(id, true);
  if (!wide.value) drawerOpen.value = true;
}
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Mail" docs="/guide/mail">
      <template #actions>
        <span :class="pillClass" :title="capture.title" data-testid="capture-pill">
          <TriangleAlert v-if="capture.tone === 'warn'" class="size-3.5" aria-hidden="true" />
          <span
            v-else
            :class="cn('size-[7px] rounded-full', capture.tone === 'ok' ? 'bg-success' : 'bg-faint')"
            aria-hidden="true"
          />
          {{ capture.label }}
        </span>
        <Button variant="outline" size="sm" @click="setupOpen = true">
          <SlidersHorizontal />
          Setup
        </Button>
        <Button
          variant="outline"
          size="icon"
          class="size-8"
          aria-label="Open in a separate window"
          title="Open in a separate window"
          @click="openWindow"
        >
          <AppWindow />
        </Button>
      </template>
    </PageHeader>

    <div class="flex min-h-0 flex-1" :data-layout="wide ? 'wide' : 'compact'">
      <div :class="wide ? 'flex w-[360px] shrink-0 flex-col pl-4 pt-2.5' : 'flex min-w-0 flex-1 flex-col pl-4 pt-2.5'">
        <MailInbox
          v-model:app="app"
          class="flex-1"
          :layout="wide ? 'list' : 'table'"
          :mails="filtered"
          :total="list.length"
          :unread="box.unread.value"
          :apps="box.apps.value"
          :selected="selectedId"
          :now="now"
          :loading="box.loading.value"
          :error="box.error.value"
          :clearing="busy === 'clear'"
          :capture="mail"
          @select="onSelect"
          @clear-all="box.clearAll"
          @retry="box.refresh"
          @copy-env="copyEnv"
          @turn-on="toggleCapture(true)"
        />
      </div>

      <div :class="wide ? 'min-w-0 flex-1 border-l' : 'hidden'" data-testid="reader-column">
        <InspectorFrame
          :mode="wide ? 'panel' : 'drawer'"
          :shown="wide ? list.length > 0 : drawerOpen && !!selectedId"
          label="Message"
          testid="mail-reader"
          backdrop-class="mail-reader-backdrop"
          wide-drawer
          @close="drawerOpen = false"
        >
          <MailReader
            :detail="detail"
            :tld="report?.tld"
            :loading="loadingDetail"
            :now="now"
            :deleting="busy === 'delete'"
            :closable="!wide"
            @delete="box.deleteMail"
            @close="drawerOpen = false"
          />
        </InspectorFrame>
      </div>
    </div>

    <MailSetupDialog
      v-model:open="setupOpen"
      v-model:from-name="fromName"
      v-model:from-address="fromAddress"
      :capture="mail"
      :env-text="envText"
      :toggling="toggling"
      @toggle="toggleCapture"
      @copy="copyEnv"
      @change-port="changePort"
    />
  </div>
</template>
