<script setup lang="ts">
import { Antenna, Search, Trash2, Pin, PinOff, X } from "lucide-vue-next";
import { computed, onMounted, ref } from "vue";

import TitleBar from "@/components/TitleBar.vue";
import Input from "@/components/ui/Input.vue";
import { useToast } from "@/composables/useToast";
import { usePoll } from "@/composables/usePoll";
import {
  clearDumps,
  deleteDump,
  dumpsStatus,
  IpcError,
  listDumps,
  pinDump,
  setDumpsEnabled,
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
});
const enabled = ref(false);
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
];

function tabCount(tab: (typeof TABS)[number]): number {
  if (!tab.countKey) {
    const c = counts.value;
    return c.dumps + c.queries + c.jobs + c.views + c.requests + c.logs + c.cache;
  }
  return counts.value[tab.countKey];
}

// Incremental fetch: append new events, drop removed ids, advance the cursor.
async function poll(): Promise<void> {
  const r = await listDumps(cursor);
  if (r.removed_ids.length) {
    const removed = new Set(r.removed_ids);
    events.value = events.value.filter((e) => !removed.has(e.id));
  }
  if (r.events.length) events.value.push(...r.events);
  cursor = r.latest_id;
  counts.value = r.counts;
}

const { refresh } = usePoll(poll, 750, { pollWhileHidden: true });

const filtered = computed(() => {
  const q = search.value.trim().toLowerCase();
  return events.value.filter((e) => {
    if (activeTab.value !== "all" && e.category !== activeTab.value) return false;
    if (!q) return true;
    return `${e.site} ${rowBody(e)} ${rowCaller(e)}`.toLowerCase().includes(q);
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

function rowCaller(e: DumpEvent): string {
  const file = str(e.payload.file);
  if (!file) return "";
  const line = str(e.payload.line);
  return line ? `${file}:${line}` : file;
}

function rowDuration(e: DumpEvent): string {
  const ms = e.payload.time_ms ?? e.payload.duration_ms;
  return ms === undefined || ms === null ? "" : `${ms} ms`;
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
      return `${str(p.method)} ${str(p.uri)} → ${str(p.status)}`.trim();
    case "cache":
      return `${str(p.event)} ${str(p.key)}`.trim();
    default:
      return JSON.stringify(p);
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

async function doClear(): Promise<void> {
  try {
    await clearDumps();
    events.value = [];
  } catch (e) {
    toast.error("Couldn't clear dumps", (e as IpcError).message);
  }
}

async function doDelete(e: DumpEvent): Promise<void> {
  try {
    await deleteDump(e.id);
    events.value = events.value.filter((x) => x.id !== e.id);
  } catch (err) {
    toast.error("Couldn't delete", (err as IpcError).message);
  }
}

async function togglePin(e: DumpEvent): Promise<void> {
  const next = !e.pinned;
  try {
    await pinDump(e.id, next);
    e.pinned = next;
  } catch (err) {
    toast.error("Couldn't pin", (err as IpcError).message);
  }
}

onMounted(async () => {
  try {
    const s = await dumpsStatus();
    enabled.value = s.enabled;
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

    <!-- Toolbar: antenna toggle, clear, filter tabs, search. -->
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
                v-if="rowDuration(e)"
                class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
              >
                {{ rowDuration(e) }}
              </span>
              <span class="truncate text-xs text-muted-foreground">{{ rowCaller(e) }}</span>

              <div class="ml-auto flex items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                <button
                  type="button"
                  :title="e.pinned ? 'Unpin' : 'Pin'"
                  class="flex size-6 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground"
                  :class="e.pinned ? 'opacity-100 text-blue-500' : ''"
                  @click="togglePin(e)"
                >
                  <component :is="e.pinned ? PinOff : Pin" class="size-3.5" />
                </button>
                <button
                  type="button"
                  title="Delete"
                  class="flex size-6 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground"
                  @click="doDelete(e)"
                >
                  <X class="size-3.5" />
                </button>
              </div>
            </div>
            <pre class="mt-1.5 whitespace-pre-wrap break-words font-mono text-xs leading-relaxed text-foreground">{{ rowBody(e) }}</pre>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
