<script lang="ts">
import { ref } from "vue";

/**
 * Whether an update is being applied. Module-scoped so the copy that mounts
 * when the window crosses the wide breakpoint mid-apply keeps Update disabled.
 */
const applyingUpdate = ref(false);
</script>

<script setup lang="ts">
import { getVersion } from "@tauri-apps/api/app";
import {
  ArrowUpRight,
  CircleCheck,
  Download,
  FileText,
  GitFork,
  Globe,
  Scale,
  Stethoscope,
} from "lucide-vue-next";
import { computed, onMounted, type Component } from "vue";

import logoUrl from "@/assets/logo.svg";
import mdbinMarkUrl from "@/assets/mdbin-mark.svg";
import UpdateActions from "@/components/settings/UpdateActions.vue";
import Button from "@/components/ui/Button.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  applyUpdate,
  cachedUpdateStatus,
  checkUpdates,
  IpcError,
  openInBrowser,
  protocolVersion,
  setUpdateChannel,
  status,
} from "@/ipc/client";
import type { UpdateChannel, UpdateStatusResponse } from "@/ipc/types";
import { updateHeadline } from "@/lib/settings";
import { humaniseAgo } from "@/lib/utils";

/**
 * Settings' About content: versions, self-update with the release channel,
 * links, and the Logs / Diagnostics entry points (the dialogs live in the
 * parent so a layout switch doesn't close them). `column` is the 302px right
 * column of the wide layout; `card` is the header card of the compact one.
 */
withDefaults(defineProps<{ layout?: "column" | "card" }>(), { layout: "column" });

const emit = defineEmits<{ logs: []; diagnostics: [] }>();

const toast = useToast();
const { connected } = useDaemon();
const running = computed(() => connected.value === true);

const appVersion = ref("");
const protocol = ref<number | null>(null);
const daemonVersion = ref("");

const updateChannel = ref<UpdateChannel>("stable");
const updateStatus = ref<UpdateStatusResponse | null>(null);
const checkingUpdates = ref(false);

const headline = computed(() =>
  updateHeadline(updateStatus.value, {
    running: running.value,
    applying: applyingUpdate.value,
  }),
);

const checkedAgo = computed(() => {
  const at = updateStatus.value?.checked_at_epoch;
  return at ? humaniseAgo(at) : null;
});

const LINKS: { label: string; url: string; icon: Component | string }[] = [
  { label: "yerd.app", url: "https://yerd.app", icon: Globe },
  { label: "GitHub", url: "https://github.com/forjedio/yerd", icon: GitFork },
  {
    label: "MIT licence",
    url: "https://github.com/forjedio/yerd/blob/main/LICENSE.md",
    icon: Scale,
  },
  { label: "mdbin.app", url: "https://mdbin.app/?utm=yerdapp", icon: mdbinMarkUrl },
];

/**
 * Set once the user triggers a live check or changes the channel, so the slower
 * cached prefetch can't clobber fresher state when it resolves later.
 */
let liveUpdateFlow = false;

/**
 * Monotonic guard for overlapping live checks. The channel picker stays enabled
 * while a check is in flight (only `!running` disables it), so a second channel
 * change / check can start before the first resolves. Each check captures the
 * current seq before awaiting and only commits if it's still the latest, so an
 * older response that resolves last can't overwrite newer state out of order.
 */
let checkSeq = 0;

/**
 * Pre-fill the section from the daemon's persisted last-check result (no
 * network) so the channel, status and "last checked" show immediately. A down
 * or older daemon leaves the section in its "Not checked" state.
 */
async function loadCachedUpdate(): Promise<void> {
  const s = await cachedUpdateStatus().catch(() => null);
  if (!s || liveUpdateFlow) return;
  updateStatus.value = s;
  updateChannel.value = s.channel;
}

/** The toast for an explicit "Check now". A `cached` source means the daemon
 *  couldn't reach the update server, so it doesn't claim "latest version". */
function notifyCheckResult(s: UpdateStatusResponse): void {
  if (s.available && s.target) {
    toast.success("Update available", `Version ${s.target} can be installed.`);
  } else if (s.ahead_of_stable) {
    toast.info("Up to date", "You're on a pre-release ahead of stable.");
  } else if (s.source === "cached") {
    toast.info(
      "Couldn't reach the update server",
      "Showing the last cached result - you may be offline.",
    );
  } else {
    toast.success("Up to date", "You're on the latest version.");
  }
}

/**
 * A live update check. Notifies only on an explicit "Check now" (`notify`); a
 * check triggered by a channel change updates the shown status silently. A
 * superseded check neither commits nor toasts, and only the latest in-flight
 * check clears the spinner.
 */
async function runUpdateCheck(notify = false): Promise<void> {
  liveUpdateFlow = true;
  checkingUpdates.value = true;
  const seq = ++checkSeq;
  try {
    const s = await checkUpdates();
    if (seq !== checkSeq) return;
    updateStatus.value = s;
    updateChannel.value = s.channel;
    if (notify) notifyCheckResult(s);
  } catch (e) {
    if (seq !== checkSeq) return;
    if (notify) toast.error("Couldn't check for updates", (e as IpcError).message);
  } finally {
    if (seq === checkSeq) checkingUpdates.value = false;
  }
}

/**
 * Download, verify and apply. The app quits during the swap and the applier
 * relaunches it, so `applyUpdate` usually never returns; a thrown error means
 * staging failed (offline, verification, or a non-writable /Applications).
 */
async function onUpdateNow(): Promise<void> {
  applyingUpdate.value = true;
  try {
    await applyUpdate(updateChannel.value);
  } catch (e) {
    applyingUpdate.value = false;
    toast.error("Couldn't apply the update", (e as IpcError).message);
  }
}

/** Persist the channel (optimistically, reverting on failure), then re-check so
 *  the shown status reflects it. */
async function onUpdateChannelChange(value: string): Promise<void> {
  if (value !== "stable" && value !== "edge") return;
  liveUpdateFlow = true;
  const previous = updateChannel.value;
  updateChannel.value = value;
  try {
    await setUpdateChannel(value);
    await runUpdateCheck();
  } catch (e) {
    updateChannel.value = previous;
    toast.error("Couldn't change the update channel", (e as IpcError).message);
  }
}

/**
 * Loads the versions and the cached update status. Outside Tauri (tests) the
 * app version stays blank; a down daemon degrades quietly (its fields read
 * "unknown" / "-"), so only an unexpected error toasts.
 */
async function loadVersions(): Promise<void> {
  appVersion.value = await getVersion().catch(() => "");
  try {
    const [p, report] = await Promise.all([protocolVersion(), status()]);
    protocol.value = p;
    daemonVersion.value = report.daemon_version;
  } catch (e) {
    const err = e as IpcError;
    if (!err.unreachable) toast.error("Couldn't load daemon info", err.message);
  }
}

onMounted(() => {
  void loadVersions();
  void loadCachedUpdate();
});
</script>

<template>
  <div v-if="layout === 'column'" data-testid="about-column">
    <div class="flex items-center gap-3.5">
      <img :src="logoUrl" alt="" class="size-11 shrink-0" />
      <div class="min-w-0">
        <p class="pt-1 font-display text-[27px] font-normal leading-none tracking-wide">YERD</p>
        <p class="mt-1 text-[12.5px] text-muted-foreground">Local PHP, without the friction.</p>
      </div>
    </div>

    <dl class="mt-5 grid grid-cols-[110px_1fr] gap-y-2 text-[12.5px]">
      <dt class="text-muted-foreground">App</dt>
      <dd class="font-mono" data-testid="about-app-version">{{ appVersion || "-" }}</dd>
      <dt class="text-muted-foreground">Daemon</dt>
      <dd class="font-mono" data-testid="about-daemon-version">{{ daemonVersion || "unknown" }}</dd>
      <dt class="text-muted-foreground">IPC protocol</dt>
      <dd class="font-mono" data-testid="about-protocol">{{ protocol ?? "-" }}</dd>
    </dl>

    <section class="mt-[22px] border-t pt-[22px]">
      <h2 class="text-[11px] font-semibold uppercase tracking-[0.6px] text-faint">Updates</h2>
      <p
        class="mt-3 flex items-center gap-2 text-[13.5px] font-semibold"
        :title="headline.tooltip ?? undefined"
        data-testid="update-headline"
      >
        <CircleCheck v-if="headline.tone === 'ok'" class="size-4 shrink-0 text-success" />
        <Download v-else-if="headline.tone === 'available'" class="size-4 shrink-0 text-brand" />
        <span :class="headline.tone === 'none' && 'text-[12.5px] font-normal text-muted-foreground'">
          {{ headline.title }}
        </span>
      </p>
      <p v-if="headline.detail" class="mt-1 text-[12.5px] text-muted-foreground">
        {{ headline.detail }}
      </p>
      <UpdateActions
        class="mt-3 flex-wrap"
        :channel="updateChannel"
        :running="running"
        :available="updateStatus?.available === true"
        :applying="applyingUpdate"
        :checking="checkingUpdates"
        @channel="onUpdateChannelChange"
        @update="onUpdateNow"
        @check="runUpdateCheck(true)"
      />
    </section>

    <section class="mt-[22px] border-t pt-[22px]">
      <h2 class="text-[11px] font-semibold uppercase tracking-[0.6px] text-faint">Links</h2>
      <ul class="mt-3 space-y-1">
        <li v-for="link in LINKS" :key="link.url">
          <button
            type="button"
            class="flex w-full items-center gap-2.5 rounded-md py-1 text-left text-[13px] font-medium hover:text-brand"
            @click="openInBrowser(link.url)"
          >
            <img
              v-if="typeof link.icon === 'string'"
              :src="link.icon"
              alt=""
              class="size-3.5 shrink-0 rounded-[3px]"
            />
            <component :is="link.icon" v-else class="size-3.5 shrink-0 text-muted-foreground" />
            <span class="flex-1">{{ link.label }}</span>
            <ArrowUpRight class="size-3.5 shrink-0 text-faint" />
          </button>
        </li>
      </ul>
    </section>

    <section class="mt-[22px] border-t pt-[22px]">
      <h2 class="text-[11px] font-semibold uppercase tracking-[0.6px] text-faint">
        Troubleshooting
      </h2>
      <div class="mt-3 flex flex-wrap gap-2">
        <Button variant="outline" size="sm" @click="emit('logs')"><FileText /> Logs</Button>
        <Button variant="outline" size="sm" @click="emit('diagnostics')">
          <Stethoscope /> Diagnostics
        </Button>
      </div>
    </section>
  </div>

  <div v-else class="overflow-hidden rounded-[10px] border bg-card" data-testid="about-card">
    <div class="flex flex-wrap items-center gap-4 px-[18px] py-4">
      <img :src="logoUrl" alt="" class="size-11 shrink-0" />
      <div class="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2">
        <div class="flex items-baseline gap-3">
          <p class="font-display text-[20px] font-normal leading-none tracking-wide">YERD</p>
          <p class="text-[12.5px] text-muted-foreground">Local PHP, without the friction.</p>
        </div>
        <div class="flex flex-wrap items-center gap-1.5 font-mono text-[11px]">
          <span class="rounded border bg-background px-2 py-0.5">
            <span class="text-faint">App</span>
            <span class="ml-1.5" data-testid="about-app-version">{{ appVersion || "-" }}</span>
          </span>
          <span class="rounded border bg-background px-2 py-0.5">
            <span class="text-faint">Daemon</span>
            <span class="ml-1.5" data-testid="about-daemon-version">{{ daemonVersion || "unknown" }}</span>
          </span>
          <span class="rounded border bg-background px-2 py-0.5">
            <span class="text-faint">IPC</span>
            <span class="ml-1.5" data-testid="about-protocol">{{ protocol ?? "-" }}</span>
          </span>
        </div>
      </div>

      <div class="ml-auto flex flex-col items-end gap-2">
        <p
          class="flex items-center gap-1.5 text-[12px] font-semibold"
          :title="headline.tooltip ?? undefined"
          data-testid="update-headline"
        >
          <CircleCheck v-if="headline.tone === 'ok'" class="size-3.5 shrink-0 text-success" />
          <Download v-else-if="headline.tone === 'available'" class="size-3.5 shrink-0 text-brand" />
          <span :class="headline.tone === 'none' && 'font-normal text-muted-foreground'">
            {{ headline.title }}
          </span>
          <span v-if="headline.tone === 'ok' && checkedAgo" class="font-normal text-faint">
            · checked {{ checkedAgo }}
          </span>
        </p>
        <p v-if="headline.tone === 'available'" class="text-[12px] text-muted-foreground">
          {{ headline.detail }}
        </p>
        <UpdateActions
          :channel="updateChannel"
          :running="running"
          :available="updateStatus?.available === true"
          :applying="applyingUpdate"
          :checking="checkingUpdates"
          @channel="onUpdateChannelChange"
          @update="onUpdateNow"
          @check="runUpdateCheck(true)"
        />
      </div>
    </div>

    <div class="flex flex-wrap items-center gap-x-4 gap-y-2 border-t px-[18px] py-2.5">
      <button
        v-for="link in LINKS"
        :key="link.url"
        type="button"
        class="flex items-center gap-1.5 text-[12.5px] font-medium hover:text-brand"
        @click="openInBrowser(link.url)"
      >
        <img
          v-if="typeof link.icon === 'string'"
          :src="link.icon"
          alt=""
          class="size-3.5 shrink-0 rounded-[3px]"
        />
        <component :is="link.icon" v-else class="size-3.5 shrink-0 text-muted-foreground" />
        {{ link.label }}
        <ArrowUpRight class="size-3 shrink-0 text-faint" />
      </button>
      <div class="ml-auto flex gap-2">
        <Button variant="outline" size="sm" @click="emit('logs')">Logs</Button>
        <Button variant="outline" size="sm" @click="emit('diagnostics')">Diagnostics</Button>
      </div>
    </div>
  </div>
</template>
