import { emit } from "@tauri-apps/api/event";
import { onMounted, onUnmounted, ref, shallowRef, watch, type Ref, type ShallowRef } from "vue";

import { useToast } from "@/composables/useToast";
import {
  clearInsights,
  dismissInsightsIssue,
  getInsightsRequest,
  insightsOverview,
  insightsStatus,
  IpcError,
  listInsightsRequests,
  setInsightsEnabled,
  setInsightsThresholds,
} from "@/ipc/client";
import type {
  InsightsOverview,
  InsightsRange,
  InsightsRequestDetail,
  InsightsRequestSummary,
  InsightsSort,
  InsightsStatusResponse,
  InsightsThresholds,
} from "@/ipc/types";
import { isRange } from "@/lib/insights";

/** The `localStorage` key holding the selected site, shared with the Dumps window. */
export const INSIGHTS_SITE_KEY = "yerd.insights.site";

/** Broadcast to the app's other windows when the selected site changes. */
export const INSIGHTS_SITE_EVENT = "yerd:insights-site-changed";

const RANGE_KEY = "yerd.insights.range";

/** How often the visible tab's data is polled. */
export const INSIGHTS_POLL_MS = 2000;

/** The page's tabs. */
export type InsightsTab = "overview" | "requests" | "issues" | "live";

/** The Overview's bottom panel tabs. */
export type RoutesTab = "routes" | "requests" | "queries";

/** The action in flight, so its control can disable itself. */
export type InsightsBusy = "enabled" | "clear" | "thresholds" | `dismiss:${string}` | null;

/** The request list's row cap. */
export const REQUEST_LIMIT = 200;

/** The site last picked in Insights, or null. */
export function storedInsightsSite(): string | null {
  try {
    return localStorage.getItem(INSIGHTS_SITE_KEY) || null;
  } catch {
    return null;
  }
}

/**
 * Make `name` the Insights site: persist it for the next page mount and
 * broadcast it so an open Dumps window follows.
 */
export function rememberInsightsSite(name: string): void {
  store(INSIGHTS_SITE_KEY, name);
  void emit(INSIGHTS_SITE_EVENT, name).catch(() => undefined);
}

function storedRange(): InsightsRange {
  try {
    const v = localStorage.getItem(RANGE_KEY);
    if (isRange(v)) return v;
  } catch {
    return "1h";
  }
  return "1h";
}

function store(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    return;
  }
}

/** The Insights state and actions. */
export interface Insights {
  site: Ref<string | null>;
  range: Ref<InsightsRange>;
  tab: Ref<InsightsTab>;
  routesTab: Ref<RoutesTab>;
  paused: Ref<boolean>;
  status: ShallowRef<InsightsStatusResponse | null>;
  overview: ShallowRef<InsightsOverview | null>;
  requests: ShallowRef<InsightsRequestSummary[]>;
  /** Whether `requests` has been loaded for the current site, range and filter. */
  requestsLoaded: Ref<boolean>;
  slowest: ShallowRef<InsightsRequestSummary[]>;
  query: Ref<string>;
  sort: Ref<InsightsSort>;
  selectedId: Ref<string | null>;
  detail: ShallowRef<InsightsRequestDetail | null>;
  detailLoading: Ref<boolean>;
  detailError: Ref<string | null>;
  error: Ref<IpcError | null>;
  busy: Ref<InsightsBusy>;
  /** Whether the selected site is recording. */
  recording: () => boolean;
  setSite: (name: string) => void;
  select: (request: { id: string; ts_ms: number } | null) => Promise<void>;
  setEnabled: (next: boolean) => Promise<void>;
  clear: () => Promise<void>;
  dismiss: (key: string, dismissed: boolean) => Promise<void>;
  /** Save the thresholds; resolves whether the daemon accepted them. */
  setThresholds: (t: InsightsThresholds) => Promise<boolean>;
  refresh: () => Promise<void>;
}

/**
 * The Insights page's state. Every 2s it polls the status plus whatever the
 * visible tab shows: the overview on Overview and Issues (and the slowest ten
 * when the Overview's Requests panel is open), the request list on Requests.
 * The Live feed tab polls nothing here (the dumps stream does its own). `paused`
 * is view-only: it stops polling and holds the data. Polling also stops while
 * the window is hidden. The site is persisted and broadcast so the Dumps window
 * follows it; changing it drops the selection and the held data.
 */
export function useInsights(): Insights {
  const toast = useToast();
  const site = ref<string | null>(storedInsightsSite());
  const range = ref<InsightsRange>(storedRange());
  const tab = ref<InsightsTab>("overview");
  const routesTab = ref<RoutesTab>("routes");
  const paused = ref(false);
  const status = shallowRef<InsightsStatusResponse | null>(null);
  const overview = shallowRef<InsightsOverview | null>(null);
  const requests = shallowRef<InsightsRequestSummary[]>([]);
  const requestsLoaded = ref(false);
  const slowest = shallowRef<InsightsRequestSummary[]>([]);
  const query = ref("");
  const sort = ref<InsightsSort>("slowest");
  const selectedId = ref<string | null>(null);
  const detail = shallowRef<InsightsRequestDetail | null>(null);
  const detailLoading = ref(false);
  const detailError = ref<string | null>(null);
  const error = ref<IpcError | null>(null);
  const busy = ref<InsightsBusy>(null);

  let timer: ReturnType<typeof setTimeout> | null = null;
  let inFlight = false;
  let disposed = false;
  let generation = 0;

  function recording(): boolean {
    const name = site.value?.toLowerCase();
    return !!name && !!status.value?.sites.includes(name);
  }

  async function load(): Promise<void> {
    const name = site.value;
    const gen = generation;
    const [st, data] = await Promise.all([insightsStatus(), name ? loadTab(name) : Promise.resolve(null)]);
    if (gen !== generation) return;
    status.value = st;
    data?.();
  }

  async function loadTab(name: string): Promise<(() => void) | null> {
    const r = range.value;
    if (tab.value === "overview" || tab.value === "issues") {
      const wantSlowest = tab.value === "overview" && routesTab.value === "requests";
      const [o, slow] = await Promise.all([
        insightsOverview(name, r),
        wantSlowest ? listInsightsRequests(name, r, "slowest", 10) : Promise.resolve(null),
      ]);
      return () => {
        overview.value = o;
        if (slow) slowest.value = slow;
      };
    }
    if (tab.value === "requests") {
      const list = await listInsightsRequests(name, r, sort.value, REQUEST_LIMIT, query.value);
      return () => {
        requests.value = list;
        requestsLoaded.value = true;
      };
    }
    return null;
  }

  async function tick(): Promise<void> {
    if (inFlight || disposed) return;
    if (paused.value || document.visibilityState === "hidden") {
      schedule();
      return;
    }
    inFlight = true;
    try {
      await load();
      error.value = null;
    } catch (e) {
      error.value = e instanceof IpcError ? e : new IpcError(String(e));
    } finally {
      inFlight = false;
      schedule();
    }
  }

  function schedule(): void {
    if (disposed) return;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => void tick(), INSIGHTS_POLL_MS);
  }

  async function refresh(): Promise<void> {
    if (timer) clearTimeout(timer);
    generation += 1;
    inFlight = false;
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

  function resetData(): void {
    generation += 1;
    overview.value = null;
    requests.value = [];
    requestsLoaded.value = false;
    slowest.value = [];
    selectedId.value = null;
    detail.value = null;
    detailError.value = null;
  }

  function setSite(name: string): void {
    if (name === site.value) return;
    site.value = name;
    rememberInsightsSite(name);
    resetData();
    void refresh();
  }

  watch(range, (r) => {
    store(RANGE_KEY, r);
    generation += 1;
    overview.value = null;
    requestsLoaded.value = false;
    void refresh();
  });

  watch([tab, routesTab], () => {
    generation += 1;
    void refresh();
  });

  watch([query, sort], () => {
    generation += 1;
    requestsLoaded.value = false;
    void refresh();
  });

  watch(paused, (p) => {
    if (!p) void refresh();
  });

  let selectToken = 0;

  async function select(request: { id: string; ts_ms: number } | null): Promise<void> {
    const token = ++selectToken;
    detailError.value = null;
    if (!request || !site.value) {
      selectedId.value = null;
      detail.value = null;
      return;
    }
    selectedId.value = request.id;
    if (detail.value?.summary.id === request.id) return;
    detail.value = null;
    detailLoading.value = true;
    try {
      const d = await getInsightsRequest(site.value, request.id, request.ts_ms);
      if (token === selectToken) detail.value = d;
    } catch (e) {
      if (token === selectToken) detailError.value = e instanceof Error ? e.message : String(e);
    } finally {
      if (token === selectToken) detailLoading.value = false;
    }
  }

  async function run(slot: Exclude<InsightsBusy, null>, title: string, action: () => Promise<void>): Promise<void> {
    if (busy.value) return;
    busy.value = slot;
    try {
      await action();
    } catch (e) {
      toast.error(title, e instanceof Error ? e.message : String(e));
    } finally {
      busy.value = null;
    }
  }

  function setEnabled(next: boolean): Promise<void> {
    const name = site.value;
    if (!name) return Promise.resolve();
    return run("enabled", next ? "Couldn't start recording" : "Couldn't stop recording", async () => {
      await setInsightsEnabled(name, next);
      await refresh();
    });
  }

  function clear(): Promise<void> {
    const name = site.value;
    if (!name) return Promise.resolve();
    return run("clear", "Couldn't clear Insights", async () => {
      await clearInsights(name);
      resetData();
      await refresh();
    });
  }

  function dismiss(key: string, dismissed: boolean): Promise<void> {
    const name = site.value;
    if (!name) return Promise.resolve();
    return run(`dismiss:${key}`, dismissed ? "Couldn't dismiss the issue" : "Couldn't restore the issue", async () => {
      await dismissInsightsIssue(name, key, dismissed);
      await refresh();
    });
  }

  async function setThresholds(t: InsightsThresholds): Promise<boolean> {
    let saved = false;
    await run("thresholds", "Couldn't save the thresholds", async () => {
      await setInsightsThresholds(t);
      saved = true;
      await refresh();
    });
    return saved;
  }

  return {
    site,
    range,
    tab,
    routesTab,
    paused,
    status,
    overview,
    requests,
    requestsLoaded,
    slowest,
    query,
    sort,
    selectedId,
    detail,
    detailLoading,
    detailError,
    error,
    busy,
    recording,
    setSite,
    select,
    setEnabled,
    clear,
    dismiss,
    setThresholds,
    refresh,
  };
}
