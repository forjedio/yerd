import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { computed, onMounted, onUnmounted, ref, type ComputedRef, type Ref } from "vue";

import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  applyUpdate,
  cachedUpdateStatus,
  checkUpdates,
  IpcError,
  setUpdateChannel,
} from "@/ipc/client";
import type { UpdateChannel, UpdateStatusResponse } from "@/ipc/types";
import { updateHeadline, type UpdateHeadline } from "@/lib/settings";
import { humaniseAgo } from "@/lib/utils";

/**
 * Whether an update is being applied. Module-scoped so it survives any About
 * section remount while the app swaps itself out.
 */
const applying = ref(false);

/** The self-update state behind Settings ▸ About, shared by both of its layouts. */
export interface UpdateCheck {
  running: ComputedRef<boolean>;
  channel: Ref<UpdateChannel>;
  status: Ref<UpdateStatusResponse | null>;
  checking: Ref<boolean>;
  applying: Ref<boolean>;
  headline: ComputedRef<UpdateHeadline>;
  /** "4 minutes ago" for the last check, or null when never checked. */
  checkedAgo: ComputedRef<string | null>;
  /** Pre-fill from the daemon's persisted last-check result (no network); a
   *  no-op once a status is shown, so a layout switch doesn't re-fetch it. */
  loadCached: () => Promise<void>;
  /** Re-read the daemon's persisted last-check result even when a status is
   *  already shown, so a result found elsewhere (the tray) replaces a stale one. */
  refreshCached: () => Promise<void>;
  /** A live check; `notify` toasts the outcome (an explicit "Check now"). */
  check: (notify?: boolean) => Promise<void>;
  /** Download, verify and apply the update on the current channel. */
  apply: () => Promise<void>;
  /** Persist a channel (optimistically, reverting on failure), then re-check. */
  changeChannel: (value: string) => Promise<void>;
}

/**
 * The About section's update flow. The Settings view owns one instance and
 * hands it to whichever About layout is showing, so crossing the wide
 * breakpoint mid-check keeps the spinner and the fresh result.
 */
export function useUpdateCheck(): UpdateCheck {
  const toast = useToast();
  const { connected } = useDaemon();
  const running = computed(() => connected.value === true);

  const channel = ref<UpdateChannel>("stable");
  const status = ref<UpdateStatusResponse | null>(null);
  const checking = ref(false);

  /**
   * Set once a live check or channel change starts, so the slower cached
   * prefetch can't clobber fresher state when it resolves later.
   */
  let liveFlow = false;

  /**
   * Monotonic guard for overlapping live checks: the channel picker stays
   * enabled mid-check, so only the latest check may commit, toast or clear the
   * spinner.
   */
  let checkSeq = 0;

  const headline = computed(() =>
    updateHeadline(status.value, { running: running.value, applying: applying.value }),
  );

  const checkedAgo = computed(() => {
    const at = status.value?.checked_at_epoch;
    return at ? humaniseAgo(at) : null;
  });

  async function loadCached(): Promise<void> {
    if (status.value || liveFlow) return;
    const s = await cachedUpdateStatus().catch(() => null);
    if (!s || liveFlow) return;
    status.value = s;
    channel.value = s.channel;
  }

  async function refreshCached(): Promise<void> {
    if (checking.value) return;
    const seq = checkSeq;
    const s = await cachedUpdateStatus().catch(() => null);
    if (!s || checking.value || seq !== checkSeq) return;
    status.value = s;
    channel.value = s.channel;
  }

  /** A `cached` source means the daemon couldn't reach the update server, so
   *  the toast doesn't claim "latest version". */
  function notifyResult(s: UpdateStatusResponse): void {
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

  async function check(notify = false): Promise<void> {
    liveFlow = true;
    checking.value = true;
    const seq = ++checkSeq;
    try {
      const s = await checkUpdates();
      if (seq !== checkSeq) return;
      status.value = s;
      channel.value = s.channel;
      if (notify) notifyResult(s);
    } catch (e) {
      if (seq !== checkSeq) return;
      if (notify) toast.error("Couldn't check for updates", (e as IpcError).message);
    } finally {
      if (seq === checkSeq) checking.value = false;
    }
  }

  /** The app quits during the swap and the applier relaunches it, so
   *  `applyUpdate` usually never returns; a thrown error means staging failed
   *  (offline, verification, or a non-writable /Applications). */
  async function apply(): Promise<void> {
    applying.value = true;
    try {
      await applyUpdate(channel.value);
    } catch (e) {
      applying.value = false;
      toast.error("Couldn't apply the update", (e as IpcError).message);
    }
  }

  async function changeChannel(value: string): Promise<void> {
    if (value !== "stable" && value !== "edge") return;
    liveFlow = true;
    const previous = channel.value;
    channel.value = value;
    try {
      await setUpdateChannel(value);
      await check();
    } catch (e) {
      channel.value = previous;
      toast.error("Couldn't change the update channel", (e as IpcError).message);
    }
  }

  return {
    running,
    channel,
    status,
    checking,
    applying,
    headline,
    checkedAgo,
    loadCached,
    refreshCached,
    check,
    apply,
    changeChannel,
  };
}

/** The route the tray's "Update Yerd" and "About" items navigate to. */
const ABOUT_ROUTE = "/about";

/**
 * Call `onRequest` whenever the tray asks for the About page while the calling
 * view is already mounted. `/about` redirects to the Settings view, so when that
 * view is already showing the navigation is a duplicate and nothing remounts:
 * this is the only signal that the tray found something worth showing. Must be
 * called from a component's setup. Outside Tauri (tests) it does nothing.
 */
export function useAboutRequests(onRequest: () => void): void {
  let unlisten: UnlistenFn | undefined;
  let disposed = false;
  onMounted(async () => {
    try {
      const off = await listen<string>("navigate", (event) => {
        if (event.payload === ABOUT_ROUTE) onRequest();
      });
      if (disposed) off();
      else unlisten = off;
    } catch {
      unlisten = undefined;
    }
  });
  onUnmounted(() => {
    disposed = true;
    unlisten?.();
  });
}
