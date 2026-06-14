<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Antenna, Layers, Pin, PinOff, Search, Trash2 } from "lucide-vue-next";
import { computed, onMounted, ref } from "vue";

import TitleBar from "@/components/TitleBar.vue";
import Input from "@/components/ui/Input.vue";
import { useToast } from "@/composables/useToast";
import { usePoll } from "@/composables/usePoll";
import {
  clearDumps,
  dumpsStatus,
  IpcError,
  listDumps,
  openInEditor,
  setDumpsEnabled,
  setDumpsPersist,
} from "@/ipc/client";
import type { DumpCategory, DumpCounts, DumpEvent } from "@/ipc/types";

const toast = useToast();

const events = ref<DumpEvent[]>([]);
const counts = ref<DumpCounts>({
  dumps: 0,
  queries: 0,
  jobs: 0,
  views: 0,
  requests: 0,
  logs: 0,
  cache: 0,
  http: 0,
});
const enabled = ref(false);
const persist = ref(false);
const alwaysOnTop = ref(false);
const activeTab = ref<DumpCategory | "all">("all");
const search = ref("");
let cursor = 0;

const TABS: { key: DumpCategory | "all"; label: string; countKey?: keyof DumpCounts }[] = [
  { key: "all", label: "All" },
  { key: "dump", label: "Dumps", countKey: "dumps" },
  { key: "query", label: "Queries", countKey: "queries" },
  { key: "job", label: "Jobs", countKey: "jobs" },
  { key: "view", label: "Views", countKey: "views" },
  { key: "request", label: "Requests", countKey: "requests" },
  { key: "log", label: "Logs", countKey: "logs" },
  { key: "cache", label: "Cache", countKey: "cache" },
  { key: "http", label: "HTTP", countKey: "http" },
];

function tabCount(tab: (typeof TABS)[number]): number {
  if (!tab.countKey) {
    const c = counts.value;
    return c.dumps + c.queries + c.jobs + c.views + c.requests + c.logs + c.cache + c.http;
  }
  return counts.value[tab.countKey];
}

// Incremental fetch: drop evicted/deleted rows, append new ones, advance cursor.
async function poll(): Promise<void> {
  const r = await listDumps(cursor);
  // Drop deleted rows AND anything below the server's min_live_id (evicted or
  // cleared) — so reconciliation never depends on the bounded removed-ids log,
  // and the client array can't outgrow the server buffer.
  const removed = new Set(r.removed_ids);
  events.value = events.value.filter((e) => e.id >= r.min_live_id && !removed.has(e.id));
  if (r.events.length) {
    // De-dup defensively so a re-sent page (e.g. after an IPC error) can't
    // double-render rows we already hold.
    const have = new Set(events.value.map((e) => e.id));
    const fresh = r.events.filter((e) => !have.has(e.id));
    if (fresh.length) events.value.push(...fresh);
  }
  cursor = r.latest_id;
  counts.value = r.counts;
}

const { refresh } = usePoll(poll, 750, { pollWhileHidden: true });

const filtered = computed<DumpEvent[]>(() => {
  const q = search.value.trim().toLowerCase();
  const tab = activeTab.value;
  return events.value.filter((e) => {
    if (tab !== "all" && e.category !== tab) return false;
    if (q === "") return true;
    const hay = `${e.category} ${e.site} ${rowBody(e)} ${rowCaller(e)} ${JSON.stringify(
      e.payload,
    )}`.toLowerCase();
    return hay.includes(q);
  });
});

interface Group {
  key: string;
  ts_ms: number;
  site: string;
  events: DumpEvent[];
}

// Newest-first, grouped by request so consecutive rows from one request share a
// header (timestamp + site), like Herd.
const groups = computed<Group[]>(() => {
  const out: Group[] = [];
  for (const e of [...filtered.value].reverse()) {
    const last = out[out.length - 1];
    if (last && last.key === e.request_id) {
      last.events.push(e);
    } else {
      out.push({ key: e.request_id, ts_ms: e.ts_ms, site: e.site, events: [e] });
    }
  }
  return out;
});

function str(v: unknown): string {
  return v === undefined || v === null ? "" : String(v);
}

// Top-level project directories used to find where the app root ends, so an
// absolute path like /Users/me/Herd/blog/app/Foo.php shows as app/Foo.php.
const PATH_ANCHORS = new Set([
  "app",
  "vendor",
  "routes",
  "config",
  "database",
  "bootstrap",
  "public",
  "resources",
  "storage",
  "tests",
  "src",
  "lib",
  "packages",
  "modules",
  "Modules",
  "nova-components",
]);

/** Strip everything before the first project-root anchor directory. */
function projectRelative(file: string): string {
  const parts = file.split("/");
  const idx = parts.findIndex((p) => PATH_ANCHORS.has(p));
  return idx >= 0 ? parts.slice(idx).join("/") : file;
}

function basename(p: string): string {
  const parts = p.split("/");
  return parts[parts.length - 1] || p;
}

/** Just the filename + line, for the badge (e.g. `PluginCache.php:36`). */
function rowCaller(e: DumpEvent): string {
  const file = str(e.payload.file);
  if (!file) return "";
  const line = str(e.payload.line);
  const name = basename(file);
  return line ? `${name}:${line}` : name;
}

/** Project-relative path + line, for the hover tooltip (e.g. `app/Foo.php:36`). */
function rowCallerRel(e: DumpEvent): string {
  const file = str(e.payload.file);
  if (!file) return "";
  const line = str(e.payload.line);
  const rel = projectRelative(file);
  return line ? `${rel}:${line}` : rel;
}

/** Open the caller's file in the OS default editor. */
async function openCaller(e: DumpEvent): Promise<void> {
  const file = str(e.payload.file);
  if (!file.startsWith("/")) return; // skip non-paths (e.g. "Command line code")
  try {
    await openInEditor(file);
  } catch (err) {
    toast.error("Couldn't open file", String(err));
  }
}

/** Format a duration in ms: 2dp ms under a second, else 2dp seconds. */
function formatDuration(ms: number): string {
  if (!Number.isFinite(ms)) return "";
  return ms >= 1000 ? `${(ms / 1000).toFixed(2)} s` : `${ms.toFixed(2)} ms`;
}

function rowDuration(e: DumpEvent): string {
  const raw = e.payload.time_ms ?? e.payload.duration_ms;
  return typeof raw === "number" ? formatDuration(raw) : "";
}

/** Affected/returned row count tag for query rows (null when unavailable). */
function rowCount(e: DumpEvent): string | null {
  if (e.category !== "query") return null;
  const rc = e.payload.row_count;
  if (typeof rc !== "number") return null;
  return `${rc} ${rc === 1 ? "row" : "rows"}`;
}

function rowBody(e: DumpEvent): string {
  const p = e.payload;
  switch (e.category) {
    case "query":
      return str(p.sql);
    case "dump":
      return str(p.value_text) || str(p.value_html);
    case "log":
      return `${str(p.level)} ${str(p.message)}`.trim();
    case "job":
    case "view":
      return str(p.name);
    case "request":
      // method + status shown as tags; the body is the URI.
      return str(p.uri);
    case "http":
      // method + status shown as tags; the body is the URL.
      return str(p.url);
    case "cache": {
      // The hit/miss/write event is shown as a coloured tag; the body is the key.
      const key = str(p.key);
      const preview = str(p.value_preview);
      return preview ? `${key} = ${preview}` : key;
    }
    default:
      return JSON.stringify(p);
  }
}

/** Blue HTTP-method tag for request/http rows, or null. */
function methodTag(e: DumpEvent): string | null {
  if (e.category !== "http" && e.category !== "request") return null;
  return str(e.payload.method).toUpperCase() || null;
}

/** Status-code tag coloured by class: 2xx green, 3xx amber, 4xx/5xx red. */
function statusTag(e: DumpEvent): { text: string; class: string } | null {
  if (e.category !== "http" && e.category !== "request") return null;
  const raw = e.payload.status;
  const code = typeof raw === "number" ? raw : Number.parseInt(str(raw), 10);
  if (!Number.isFinite(code) || code <= 0) return null;
  let cls = "bg-muted text-muted-foreground";
  if (code >= 200 && code < 300) cls = "bg-green-500/15 text-green-600 dark:text-green-400";
  else if (code >= 300 && code < 400) cls = "bg-amber-500/15 text-amber-600 dark:text-amber-400";
  else if (code >= 400) cls = "bg-red-500/15 text-red-600 dark:text-red-400";
  return { text: String(code), class: cls };
}

/** Coloured tag for a cache event's hit/miss/write/forget, or null. */
function cacheTag(e: DumpEvent): { text: string; class: string } | null {
  if (e.category !== "cache") return null;
  switch (str(e.payload.event).toLowerCase()) {
    case "hit":
      return { text: "HIT", class: "bg-green-500/15 text-green-600 dark:text-green-400" };
    case "missed":
    case "miss":
      return { text: "MISS", class: "bg-red-500/15 text-red-600 dark:text-red-400" };
    case "written":
    case "write":
      return { text: "WRITE", class: "bg-blue-500/15 text-blue-600 dark:text-blue-400" };
    case "forgotten":
    case "forget":
      return { text: "FORGET", class: "bg-amber-500/15 text-amber-600 dark:text-amber-400" };
    default: {
      const ev = str(e.payload.event);
      return ev ? { text: ev.toUpperCase(), class: "bg-muted text-muted-foreground" } : null;
    }
  }
}

const CATEGORY_LABEL: Record<DumpCategory, string> = {
  dump: "DUMP",
  query: "QUERY",
  job: "JOB",
  view: "VIEW",
  request: "REQUEST",
  log: "LOG",
  cache: "CACHE",
  http: "HTTP",
};

function formatTime(ms: number): string {
  if (!ms) return "";
  return new Date(ms).toLocaleTimeString();
}

async function toggleEnabled(): Promise<void> {
  const next = !enabled.value;
  try {
    await setDumpsEnabled(next);
    enabled.value = next;
  } catch (e) {
    toast.error("Couldn't toggle interception", (e as IpcError).message);
  }
}

async function togglePersist(): Promise<void> {
  const next = !persist.value;
  try {
    await setDumpsPersist(next);
    persist.value = next;
  } catch (e) {
    toast.error("Couldn't toggle persist", (e as IpcError).message);
  }
}

async function toggleAlwaysOnTop(): Promise<void> {
  const next = !alwaysOnTop.value;
  try {
    await getCurrentWindow().setAlwaysOnTop(next);
    alwaysOnTop.value = next;
  } catch (e) {
    toast.error("Couldn't pin window", String(e));
  }
}

async function doClear(): Promise<void> {
  try {
    await clearDumps();
    events.value = [];
  } catch (e) {
    toast.error("Couldn't clear dumps", (e as IpcError).message);
  }
}

onMounted(async () => {
  try {
    const s = await dumpsStatus();
    enabled.value = s.enabled;
    persist.value = s.persist;
    counts.value = s.counts;
  } catch {
    // status is best-effort; the poll loop still streams events.
  }
  await refresh();
});
</script>

<template>
  <div class="flex h-full w-full flex-col overflow-hidden bg-background text-foreground">
    <TitleBar />

    <!-- Toolbar: antenna, persist, window-pin, clear, filter tabs, search. -->
    <div class="flex flex-col gap-2 border-b px-3 py-2">
      <div class="flex items-center gap-2">
        <button
          type="button"
          :title="enabled ? 'Interception on — click to pause' : 'Interception off — click to enable'"
          class="flex size-7 items-center justify-center rounded-md transition-colors"
          :class="
            enabled
              ? 'bg-green-500/15 text-green-600 dark:text-green-400'
              : 'text-muted-foreground hover:bg-accent'
          "
          @click="toggleEnabled"
        >
          <Antenna class="size-4" :class="enabled ? 'animate-pulse' : ''" />
        </button>
        <button
          type="button"
          :title="persist ? 'Persist on — keeping logs across requests' : 'Persist off — clears on each new request'"
          class="flex size-7 items-center justify-center rounded-md transition-colors"
          :class="
            persist
              ? 'bg-blue-500/15 text-blue-600 dark:text-blue-400'
              : 'text-muted-foreground hover:bg-accent'
          "
          @click="togglePersist"
        >
          <Layers class="size-4" />
        </button>
        <button
          type="button"
          :title="alwaysOnTop ? 'Window pinned on top — click to unpin' : 'Keep window on top'"
          class="flex size-7 items-center justify-center rounded-md transition-colors"
          :class="
            alwaysOnTop
              ? 'bg-amber-500/15 text-amber-600 dark:text-amber-400'
              : 'text-muted-foreground hover:bg-accent'
          "
          @click="toggleAlwaysOnTop"
        >
          <component :is="alwaysOnTop ? PinOff : Pin" class="size-4" />
        </button>
        <button
          type="button"
          title="Clear all"
          class="flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          @click="doClear"
        >
          <Trash2 class="size-4" />
        </button>

        <div class="relative ml-auto max-w-xs flex-1">
          <Search
            class="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
          />
          <Input v-model="search" placeholder="Filter…" class="pl-8" />
        </div>
      </div>

      <div class="flex flex-wrap items-center gap-1">
        <button
          v-for="tab in TABS"
          :key="tab.key"
          type="button"
          class="rounded-md px-2.5 py-1 text-xs font-medium transition-colors"
          :class="
            activeTab === tab.key
              ? 'bg-primary text-primary-foreground'
              : 'text-muted-foreground hover:bg-accent hover:text-foreground'
          "
          @click="activeTab = tab.key"
        >
          {{ tab.label }} ({{ tabCount(tab) }})
        </button>
      </div>
    </div>

    <!-- Event stream. -->
    <div class="flex-1 overflow-y-auto p-3">
      <p
        v-if="groups.length === 0"
        class="mt-10 text-center text-sm text-muted-foreground"
      >
        {{ enabled ? "Waiting for dumps…" : "Interception is off. Enable the antenna to capture." }}
      </p>

      <div v-for="group in groups" :key="group.key + '-' + group.ts_ms" class="mb-4">
        <div class="mb-1.5 flex items-center gap-2 text-xs text-muted-foreground">
          <span class="font-medium text-foreground">{{ formatTime(group.ts_ms) }}</span>
          <span v-if="group.site">· {{ group.site }}</span>
        </div>

        <div class="space-y-1.5">
          <div
            v-for="e in group.events"
            :key="e.id"
            class="group rounded-md border bg-card/60 p-2.5"
          >
            <div class="flex items-center gap-2">
              <span
                class="rounded bg-muted px-1.5 py-0.5 text-[10px] font-semibold tracking-wide text-muted-foreground"
              >
                {{ CATEGORY_LABEL[e.category] }}
              </span>
              <span
                v-if="cacheTag(e)"
                class="rounded px-1.5 py-0.5 text-[10px] font-semibold tracking-wide"
                :class="cacheTag(e)!.class"
              >
                {{ cacheTag(e)!.text }}
              </span>
              <span
                v-if="rowDuration(e)"
                class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
              >
                {{ rowDuration(e) }}
              </span>
              <span
                v-if="rowCount(e)"
                class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
              >
                {{ rowCount(e) }}
              </span>
              <span
                v-if="methodTag(e)"
                class="rounded bg-blue-500/15 px-1.5 py-0.5 text-[10px] font-semibold tracking-wide text-blue-600 dark:text-blue-400"
              >
                {{ methodTag(e) }}
              </span>
              <span
                v-if="statusTag(e)"
                class="rounded px-1.5 py-0.5 text-[10px] font-semibold tracking-wide"
                :class="statusTag(e)!.class"
              >
                {{ statusTag(e)!.text }}
              </span>
              <!-- Caller badge: filename:line, project-relative path on hover,
                   click to open in the default editor. -->
              <button
                v-if="rowCaller(e)"
                type="button"
                class="ml-auto shrink-0 cursor-pointer rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                :title="`${rowCallerRel(e)} — click to open`"
                @click="openCaller(e)"
              >
                {{ rowCaller(e) }}
              </button>
            </div>
            <pre class="mt-3 whitespace-pre-wrap break-words font-mono text-xs leading-relaxed text-foreground">{{ rowBody(e) }}</pre>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
