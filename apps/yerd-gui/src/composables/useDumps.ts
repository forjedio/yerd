import { computed, ref, watch, type ComputedRef, type Ref } from "vue";

import { usePoll } from "@/composables/usePoll";
import { invalidate } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import {
  clearDumps,
  dumpsStatus,
  IpcError,
  listDumps,
  setDumpFeature,
  setDumpsEnabled,
  setDumpsPersist,
} from "@/ipc/client";
import type { DumpEvent, DumpsStatusResponse } from "@/ipc/types";
import {
  type ChipKey,
  DUMP_SIGNALS,
  type DumpRequest,
  filterRequests,
  groupRequests,
  mergePage,
  nextSelection,
  siteMatches,
  streamRestarted,
} from "@/lib/dumps";

/** Options for {@link useDumps}. */
export interface DumpsOptions {
  /** Keep streaming while the window is hidden (the Dumps window). */
  pollWhileHidden: boolean;
  /** Whether to keep the newest request selected on its own. A getter, so a
   *  window resize can turn it on and off. */
  follow: () => boolean;
  /** Keep only events whose `site` is this Yerd site name (the Insights site),
   *  or its `{name}.{tld}` host from older extensions. A getter, so the scope can
   *  follow the picked site; null keeps every event. */
  site?: () => string | null;
  /** The local TLD for the host form of `site`. Defaults to `test`. */
  tld?: () => string;
  /** While true the stream stops polling and the list holds still. */
  paused?: () => boolean;
}

/** The action in flight, so its control can disable itself. */
export type DumpsBusy = "enabled" | "persist" | "clear" | `feature:${string}` | null;

/** The dumps state and actions shared by the Dumps screen and the Dumps window. */
export interface Dumps {
  events: Ref<DumpEvent[]>;
  /** Every held request, newest first. */
  requests: ComputedRef<DumpRequest[]>;
  /** The requests the event filter and category chip keep. */
  visible: ComputedRef<DumpRequest[]>;
  status: Ref<DumpsStatusResponse | null>;
  selectedKey: Ref<string | null>;
  selected: ComputedRef<DumpRequest | null>;
  /** The detail's category chip, kept across requests. */
  category: Ref<ChipKey>;
  /** The event filter text. */
  query: Ref<string>;
  busy: Ref<DumpsBusy>;
  select: (key: string | null) => void;
  /** Reset the event filter and the category chip. */
  clearFilter: () => void;
  setEnabled: (next: boolean) => Promise<void>;
  setPersist: (next: boolean) => Promise<void>;
  setFeature: (key: string, next: boolean) => Promise<void>;
  clear: () => Promise<void>;
  refresh: () => Promise<void>;
}

/**
 * Stream the dump buffer incrementally every 750ms and the dump server status
 * every 2.5s, group the events into requests and track the selection, the
 * category chip and the event filter. While `follow` is on, the newest visible
 * request is selected on load and a newer visible arrival takes the selection
 * as long as the newest visible was selected; an older pick stays put, and a
 * request the filter hides never takes it. Changing the filter itself never
 * moves the selection. A selection whose request is
 * evicted moves to the newest visible (following) or clears. A page fetched
 * before a clear is dropped, so it can't bring the cleared events back for a
 * tick. A daemon restart (ids start over) drops the held events and re-reads
 * the stream from the start. While `paused`, the stream skips its fetches and
 * picks up from the cursor on resume.
 */
export function useDumps(options: DumpsOptions): Dumps {
  const toast = useToast();
  const events = ref<DumpEvent[]>([]);
  let cursor = 0;
  let generation = 0;

  const stream = usePoll(
    async () => {
      if (options.paused?.()) return;
      const fetchedIn = generation;
      let page = await listDumps(cursor);
      if (fetchedIn !== generation) return;
      if (streamRestarted(cursor, page)) {
        events.value = [];
        cursor = 0;
        page = await listDumps(0);
        if (fetchedIn !== generation) return;
      }
      events.value = mergePage(events.value, page);
      cursor = page.latest_id;
    },
    750,
    { pollWhileHidden: options.pollWhileHidden },
  );
  const { data: status, refresh: refreshStatus } = usePoll<DumpsStatusResponse>(dumpsStatus, 2500, {
    pollWhileHidden: options.pollWhileHidden,
  });

  const category = ref<ChipKey>("all");
  const query = ref("");
  const scoped = computed(() => {
    const site = options.site?.() ?? null;
    if (site === null) return events.value;
    const tld = options.tld?.() ?? "test";
    return events.value.filter((e) => siteMatches(e.site, site, tld));
  });
  const requests = computed(() => groupRequests(scoped.value));
  const visible = computed(() => filterRequests(requests.value, query.value, category.value));

  const selectedKey = ref<string | null>(null);
  const selected = computed(() => requests.value.find((r) => r.key === selectedKey.value) ?? null);
  let prevFirst: string | null = null;

  function sync(): void {
    const all = requests.value;
    const shown = visible.value;
    if (options.follow()) {
      selectedKey.value = nextSelection(prevFirst, selectedKey.value, shown, all);
    } else if (selectedKey.value && !all.some((r) => r.key === selectedKey.value)) {
      selectedKey.value = null;
    }
    prevFirst = shown[0]?.key ?? null;
  }

  watch(requests, sync, { immediate: true });
  watch([query, category], () => {
    prevFirst = visible.value[0]?.key ?? null;
  });
  watch(options.follow, (on) => {
    if (on) sync();
  });
  watch(
    () => options.paused?.() ?? false,
    (p) => {
      if (!p) void stream.refresh();
    },
  );

  function select(key: string | null): void {
    selectedKey.value = key;
  }

  function clearFilter(): void {
    query.value = "";
    category.value = "all";
  }

  const busy = ref<DumpsBusy>(null);

  async function run(slot: Exclude<DumpsBusy, null>, title: string, action: () => Promise<void>): Promise<void> {
    if (busy.value) return;
    busy.value = slot;
    try {
      await action();
    } catch (e) {
      toast.error(title, (e as IpcError).message);
    } finally {
      busy.value = null;
    }
  }

  function setEnabled(next: boolean): Promise<void> {
    return run("enabled", next ? "Couldn't turn on interception" : "Couldn't pause interception", async () => {
      await setDumpsEnabled(next);
      void invalidate("dumps-status");
      await refreshStatus();
    });
  }

  function setPersist(next: boolean): Promise<void> {
    return run("persist", "Couldn't change Keep history", async () => {
      await setDumpsPersist(next);
      await refreshStatus();
    });
  }

  function setFeature(key: string, next: boolean): Promise<void> {
    const label = DUMP_SIGNALS.find((s) => s.key === key)?.label ?? key;
    return run(`feature:${key}`, `Couldn't update ${label}`, async () => {
      await setDumpFeature(key, next);
      await refreshStatus();
    });
  }

  function clear(): Promise<void> {
    return run("clear", "Couldn't clear dumps", async () => {
      await clearDumps();
      generation += 1;
      events.value = [];
    });
  }

  async function refresh(): Promise<void> {
    await Promise.all([stream.refresh(), refreshStatus()]);
  }

  return {
    events,
    requests,
    visible,
    status,
    selectedKey,
    selected,
    category,
    query,
    busy,
    select,
    clearFilter,
    setEnabled,
    setPersist,
    setFeature,
    clear,
    refresh,
  };
}
