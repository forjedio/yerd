import { onMounted, onUnmounted, ref, shallowRef, type Ref } from "vue";

import { IpcError } from "@/ipc/client";

export interface PollHandle<T> {
  data: Ref<T | null>;
  error: Ref<IpcError | null>;
  loading: Ref<boolean>;
  /** Run the fetch once now (also resets the interval cadence). */
  refresh: () => Promise<void>;
}

/**
 * Poll `fn` every `intervalMs` while the component is mounted.
 *
 * Discipline that matters for the daemon (each `status` call reads the trust
 * store and live FPM state — see the plan's "poll cost" note):
 *   - never overlaps in-flight calls,
 *   - pauses entirely while the document is hidden (background tab / tray),
 *   - stops on unmount (no leaked timers).
 *
 * Default cadence is 4s; callers should not go below ~3s for `status`.
 *
 * `pollWhileHidden` keeps polling when the document is hidden — needed for the
 * standalone dumps window, which is usually unfocused (and reports `hidden` when
 * minimised/occluded) yet must keep streaming. Default `false` so the shared
 * status poller is unaffected.
 */
export function usePoll<T>(
  fn: () => Promise<T>,
  intervalMs = 4000,
  options: { pollWhileHidden?: boolean } = {},
): PollHandle<T> {
  const { pollWhileHidden = false } = options;
  const data = shallowRef<T | null>(null);
  const error = ref<IpcError | null>(null);
  const loading = ref(false);

  let timer: ReturnType<typeof setTimeout> | null = null;
  let inFlight = false;
  let disposed = false;

  async function tick(): Promise<void> {
    if (inFlight || disposed) return;
    if (!pollWhileHidden && document.visibilityState === "hidden") {
      schedule();
      return;
    }
    inFlight = true;
    loading.value = true;
    try {
      data.value = await fn();
      error.value = null;
    } catch (e) {
      error.value = e instanceof IpcError ? e : new IpcError(String(e));
    } finally {
      inFlight = false;
      loading.value = false;
      schedule();
    }
  }

  function schedule(): void {
    if (disposed) return;
    if (timer) clearTimeout(timer);
    timer = setTimeout(tick, intervalMs);
  }

  async function refresh(): Promise<void> {
    if (timer) clearTimeout(timer);
    await tick();
  }

  function onVisible(): void {
    if (document.visibilityState === "visible") void refresh();
  }

  onMounted(() => {
    document.addEventListener("visibilitychange", onVisible);
    void tick();
  });

  onUnmounted(() => {
    disposed = true;
    if (timer) clearTimeout(timer);
    document.removeEventListener("visibilitychange", onVisible);
  });

  return { data, error, loading, refresh };
}
