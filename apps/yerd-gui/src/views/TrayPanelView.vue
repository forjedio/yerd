<script setup lang="ts">
/**
 * Frameless tray popup: site autocomplete + per-site actions + service controls.
 * Standalone window (label `tray-panel`); owns its own lightweight poll (4s,
 * paused while hidden, same discipline as `usePoll`).
 *
 * Window chrome: Minimize hides the panel; Expand opens the main dashboard.
 */
import { listen, emit, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  AlertTriangle,
  ChevronDown,
  ChevronRight,
  ExternalLink,
  FolderOpen,
  Mail,
  Maximize2,
  Minus,
  MoreHorizontal,
  Play,
  RotateCw,
  Search,
  Shield,
  Square,
  SquareTerminal,
  Star,
  Stethoscope,
  Terminal,
} from "lucide-vue-next";
import { computed, nextTick, onMounted, onUnmounted, ref, watch, type Component } from "vue";

import { useToast } from "@/composables/useToast";

import logoUrl from "@/assets/logo.svg";
import Button from "@/components/ui/Button.vue";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import {
  getTrayPreferences,
  listMails,
  listSites,
  openInBrowser,
  openPath,
  openSiteInIde,
  openTerminalAt,
  recordTrayRecent,
  restartDaemon,
  restartPhp,
  restartService,
  setTrayFavorite,
  showMailsWindow,
  startDaemon,
  startService,
  status,
  stopDaemon,
  stopService,
  IpcError,
  type TrayPreferences,
} from "@/ipc/client";
import type { MailSummary, SiteEntry, StatusReport } from "@/ipc/types";
import { siteUrl } from "@/lib/siteUrl";
import {
  trayAlerts,
  trayServiceRows,
  type TrayAlert,
  type TrayServiceRow,
} from "@/lib/trayHealth";
import {
  buildTraySiteSuggestions,
  type TraySiteSuggestion,
} from "@/lib/traySiteAutocomplete";
import { poolStateLabel } from "@/lib/utils";

const win = getCurrentWindow();
const toast = useToast();
const query = ref("");
const selected = ref(0);
const input = ref<HTMLInputElement | null>(null);
const sites = ref<SiteEntry[]>([]);
const report = ref<StatusReport | null>(null);
const prefs = ref<TrayPreferences>({ favorites: [], recent: [], trayUnavailable: false });
const busy = ref<string | null>(null);
/** Keep action icons animating long enough to read on fast IPC round-trips. */
const MIN_BUSY_MS = 400;
const recentMail = ref<MailSummary[]>([]);
/** Services / Activity start collapsed; Sites starts expanded. */
const sitesOpen = ref(true);
const servicesOpen = ref(false);
const activityOpen = ref(false);

let pollTimer: ReturnType<typeof setInterval> | undefined;
let unlistenOpen: UnlistenFn | undefined;

/** Same cadence as `usePoll` default; paused while the panel window is hidden. */
const POLL_INTERVAL_MS = 4000;

function stopPoll(): void {
  if (pollTimer !== undefined) {
    clearInterval(pollTimer);
    pollTimer = undefined;
  }
}

function startPoll(): void {
  stopPoll();
  pollTimer = setInterval(() => {
    if (document.visibilityState === "hidden") return;
    void refresh();
  }, POLL_INTERVAL_MS);
}

function onVisibility(): void {
  if (document.visibilityState === "hidden") {
    stopPoll();
  } else {
    void refresh();
    startPoll();
  }
}

const tld = computed(() => report.value?.tld ?? "test");
const serviceRows = computed(() => (report.value ? trayServiceRows(report.value) : []));
const alerts = computed(() => trayAlerts(report.value));
const showActivity = computed(() => alerts.value.length > 0 || recentMail.value.length > 0);
const activityCount = computed(() => alerts.value.length + recentMail.value.length);

const suggestions = computed(() =>
  buildTraySiteSuggestions(sites.value, query.value, {
    favorites: prefs.value.favorites,
    recent: prefs.value.recent,
    tld: tld.value,
  }),
);

const flat = computed(() => suggestions.value);
const groups = computed(() => {
  const by = new Map<string, TraySiteSuggestion[]>();
  for (const s of suggestions.value) {
    const list = by.get(s.group) ?? [];
    list.push(s);
    by.set(s.group, list);
  }
  return (["Favorites", "Recent", "Sites"] as const)
    .filter((g) => by.has(g))
    .map((title) => ({ title, items: by.get(title)! }));
});

watch(flat, () => {
  selected.value = 0;
});

/** Typing in search expands the sites list so matches are visible. */
watch(query, (q) => {
  if (q.trim()) sitesOpen.value = true;
});

async function refresh(): Promise<void> {
  try {
    report.value = await status();
  } catch {
    report.value = null;
  }
  try {
    sites.value = await listSites();
  } catch {
    /* keep last-good */
  }
  try {
    prefs.value = await getTrayPreferences();
  } catch {
    /* keep last-good */
  }
  try {
    const mails = await listMails();
    recentMail.value = mails.slice(0, 3);
  } catch {
    /* keep last-good */
  }
}

async function focusInput(): Promise<void> {
  query.value = "";
  selected.value = 0;
  await nextTick();
  input.value?.focus();
}

const stopDaemonOpen = ref(false);
async function confirmStopDaemon(close: () => void): Promise<void> {
  busy.value = "daemon";
  close();
  try {
    await stopDaemon();
    toast.success("Stopping daemon…");
  } catch (e) {
    toast.error("Couldn't stop the daemon", (e as IpcError).message);
  } finally {
    busy.value = null;
    await refresh();
  }
}

const restartDaemonOpen = ref(false);
async function confirmRestartDaemon(close: () => void): Promise<void> {
  busy.value = "restart:daemon";
  close();
  try {
    await restartDaemon();
    toast.info("Restarting daemon…", "It returns in a few seconds.");
  } catch (e) {
    toast.error("Couldn't restart the daemon", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function startDaemonNow(): Promise<void> {
  busy.value = "start:daemon";
  try {
    await startDaemon();
    toast.success("Starting daemon…");
  } catch (e) {
    toast.error("Couldn't start the daemon", (e as IpcError).message);
  } finally {
    busy.value = null;
    await refresh();
  }
}

/** Minimize / Esc: hide the tray panel. */
async function hide(): Promise<void> {
  try {
    await win.hide();
  } catch {
    /* ignore */
  }
}

async function withBusy(key: string, fn: () => Promise<void>): Promise<void> {
  busy.value = key;
  const started = Date.now();
  try {
    await fn();
  } finally {
    const remaining = MIN_BUSY_MS - (Date.now() - started);
    if (remaining > 0) {
      await new Promise((resolve) => setTimeout(resolve, remaining));
    }
    busy.value = null;
  }
}

type ServiceAction = "start" | "stop" | "restart";

function rowBusyAction(rowId: string): ServiceAction | null {
  if (!busy.value) return null;
  for (const action of ["start", "stop", "restart"] as const) {
    if (busy.value === `${action}:${rowId}`) return action;
  }
  return null;
}

function isRowBusy(rowId: string): boolean {
  return rowBusyAction(rowId) !== null;
}

async function remember(name: string): Promise<void> {
  try {
    prefs.value = await recordTrayRecent(name);
  } catch {
    /* ignore */
  }
}

async function openMain(path = "/overview"): Promise<void> {
  try {
    await invoke("show_main_window");
  } catch {
    /* optional */
  }
  await emit("navigate", path);
  await hide();
}

/** Expand: open the main dashboard panel. */
async function expand(): Promise<void> {
  await openMain("/overview");
}

async function openSite(s: SiteEntry): Promise<void> {
  await remember(s.name);
  await openInBrowser(siteUrl(s, report.value));
  await hide();
}

async function openIde(s: SiteEntry): Promise<void> {
  busy.value = `ide:${s.name}`;
  try {
    await remember(s.name);
    await openSiteInIde(s.document_root);
  } catch (e) {
    toast.error(`Couldn't open ${s.name} in your IDE`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function openTerm(s: SiteEntry): Promise<void> {
  busy.value = `term:${s.name}`;
  try {
    await remember(s.name);
    await openTerminalAt(s.document_root);
  } catch (e) {
    toast.error(`Couldn't open a terminal for ${s.name}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function reveal(s: SiteEntry): Promise<void> {
  await openPath(s.document_root);
}

async function restartPool(s: SiteEntry): Promise<void> {
  await withBusy(`php:${s.name}`, async () => {
    try {
      await restartPhp(s.php);
      await refresh();
      toast.success(`Restarted PHP ${s.php} pool`);
    } catch (e) {
      toast.error(`Couldn't restart PHP ${s.php} pool`, (e as IpcError).message);
    }
  });
}

async function toggleFav(s: SiteEntry): Promise<void> {
  const on = !prefs.value.favorites.includes(s.name);
  prefs.value = await setTrayFavorite(s.name, on);
}

async function newSite(): Promise<void> {
  try {
    await invoke("show_main_window");
  } catch {
    /* optional */
  }
  await emit("sites-intent", "create");
  await hide();
}

async function openInspect(): Promise<void> {
  await openMain("/dumps");
}

async function openDoctor(): Promise<void> {
  await openMain("/doctor");
}

async function openAlert(alert: TrayAlert): Promise<void> {
  if (alert.action === "mail") {
    await openMail();
    return;
  }
  const paths: Record<TrayAlert["action"], string> = {
    doctor: "/doctor",
    services: "/services",
    settings: "/general",
    mail: "/mail",
  };
  await openMain(paths[alert.action]);
}

async function openMail(): Promise<void> {
  try {
    await invoke("show_main_window");
  } catch {
    /* optional */
  }
  try {
    await showMailsWindow();
  } catch {
    await emit("navigate", "/mail");
  }
  await hide();
}

async function doStart(row: TrayServiceRow): Promise<void> {
  if (!row.canStart || row.kind !== "managed") return;
  servicesOpen.value = true;
  await withBusy(`start:${row.id}`, async () => {
    try {
      await startService(row.id);
      await refresh();
      toast.success(`Started ${row.label}`);
    } catch (e) {
      toast.error(`Couldn't start ${row.label}`, (e as IpcError).message);
    }
  });
}

async function doStop(row: TrayServiceRow): Promise<void> {
  if (!row.canStop || row.kind !== "managed") return;
  servicesOpen.value = true;
  await withBusy(`stop:${row.id}`, async () => {
    try {
      await stopService(row.id);
      await refresh();
      toast.success(`Stopped ${row.label}`);
    } catch (e) {
      toast.error(`Couldn't stop ${row.label}`, (e as IpcError).message);
    }
  });
}

async function doRestart(row: TrayServiceRow): Promise<void> {
  if (!row.canRestart) return;
  servicesOpen.value = true;
  await withBusy(`restart:${row.id}`, async () => {
    try {
      if (row.kind === "php_pool" && row.phpVersion) {
        await restartPhp(row.phpVersion);
      } else if (row.kind === "managed") {
        if (row.state === "stopped") {
          await startService(row.id);
        } else {
          await restartService(row.id);
        }
      }
      await refresh();
      if (row.kind === "php_pool") {
        toast.success(`Restarted PHP ${row.phpVersion} pool`);
      } else {
        toast.success(
          row.state === "stopped" ? `Started ${row.label}` : `Restarted ${row.label}`,
        );
      }
    } catch (e) {
      const label =
        row.kind === "php_pool" && row.phpVersion
          ? `PHP ${row.phpVersion} pool`
          : row.label;
      toast.error(`Couldn't restart ${label}`, (e as IpcError).message);
    }
  });
}

function alertIcon(alert: TrayAlert): Component {
  if (alert.action === "settings") return Shield;
  if (alert.action === "doctor") return Stethoscope;
  return AlertTriangle;
}

function formatMailTime(epoch: number): string {
  if (!epoch) return "";
  return new Date(epoch * 1000).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function chooseHighlighted(): void {
  const s = flat.value[selected.value];
  if (s) void openSite(s.site);
}

function move(delta: number): void {
  const n = flat.value.length;
  if (n === 0) return;
  selected.value = (selected.value + delta + n) % n;
}

function onKey(e: KeyboardEvent): void {
  if (e.key === "Escape") {
    e.preventDefault();
    void hide();
    return;
  }
  if (e.key === "ArrowDown") {
    e.preventDefault();
    move(1);
    return;
  }
  if (e.key === "ArrowUp") {
    e.preventDefault();
    move(-1);
    return;
  }
  if (e.key === "Enter") {
    e.preventDefault();
    chooseHighlighted();
  }
}

onMounted(async () => {
  await refresh();
  await focusInput();
  if (document.visibilityState !== "hidden") startPoll();
  document.addEventListener("visibilitychange", onVisibility);
  try {
    unlistenOpen = await listen("tray-panel-opened", () => {
      void refresh().then(() => focusInput());
    });
  } catch {
    /* ignore */
  }
  window.addEventListener("keydown", onKey);
});

onUnmounted(() => {
  stopPoll();
  document.removeEventListener("visibilitychange", onVisibility);
  unlistenOpen?.();
  window.removeEventListener("keydown", onKey);
});
</script>

<template>
  <div
    class="flex h-full flex-col overflow-hidden rounded-xl border bg-background/95 text-foreground shadow-xl backdrop-blur"
  >
    <div class="flex items-center justify-between gap-2 border-b px-3 py-2">
      <div class="flex min-w-0 items-center gap-2">
        <div class="flex shrink-0 items-center gap-2">
          <img :src="logoUrl" alt="" class="size-6 rounded-[7px]" />
          <span
            class="relative top-px font-display text-base font-normal leading-none tracking-wide"
            >YERD</span
          >
        </div>
      </div>
      <div class="flex shrink-0 items-center gap-1">
        <template v-if="report">
          <Button
            variant="outline"
            size="sm"
            class="h-7 px-2"
            title="Stop daemon"
            aria-label="Stop daemon"
            :disabled="busy !== null"
            @click="stopDaemonOpen = true"
          >
            <Spinner v-if="busy === 'daemon'" class="size-3.5" />
            <Square v-else class="size-3.5" />
            Stop
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="h-7 px-2"
            title="Restart daemon"
            aria-label="Restart daemon"
            :disabled="busy !== null"
            @click="restartDaemonOpen = true"
          >
            <Spinner v-if="busy === 'restart:daemon'" class="size-3.5" />
            <RotateCw v-else class="size-3.5" />
            Restart
          </Button>
        </template>
        <Button
          v-else
          variant="outline"
          size="sm"
          class="h-7 px-2"
          title="Start daemon"
          aria-label="Start daemon"
          :disabled="busy !== null"
          @click="void startDaemonNow()"
        >
          <Spinner v-if="busy === 'start:daemon'" class="size-3.5" />
          <Play v-else class="size-3.5" />
          Start
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="size-7"
          title="Minimize"
          aria-label="Minimize"
          @click="hide"
        >
          <Minus class="size-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="size-7"
          title="Open Dashboard"
          aria-label="Open Dashboard"
          @click="expand"
        >
          <Maximize2 class="size-3.5" />
        </Button>
      </div>
    </div>

    <div class="border-b px-3 py-2">
      <div class="relative">
        <Search
          class="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
        />
        <input
          ref="input"
          v-model="query"
          type="text"
          placeholder="Filter by domain…"
          class="w-full rounded-md border bg-card py-1.5 pl-7 pr-2 text-sm outline-none ring-brand focus:ring-1"
          autocomplete="off"
          spellcheck="false"
        />
      </div>
    </div>

    <div
      class="flex min-h-0 flex-col overflow-hidden"
      :class="sitesOpen ? 'flex-1' : 'shrink-0'"
    >
      <button
        type="button"
        class="flex w-full shrink-0 items-center gap-1.5 px-3 py-2 text-left hover:bg-accent/40"
        :aria-expanded="sitesOpen"
        @click="sitesOpen = !sitesOpen"
      >
        <component
          :is="sitesOpen ? ChevronDown : ChevronRight"
          class="size-3.5 shrink-0 text-muted-foreground"
        />
        <span class="text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
          Sites
        </span>
        <span class="ml-auto font-mono text-[10px] text-muted-foreground">
          {{ flat.length }}
        </span>
      </button>
      <div v-if="sitesOpen" class="min-h-0 flex-1 overflow-y-auto">
        <div
          v-if="flat.length === 0"
          class="px-3 py-8 text-center text-sm text-muted-foreground"
        >
          {{ query.trim() ? "No sites match…" : "No sites yet" }}
        </div>
        <template v-for="g in groups" :key="g.title">
          <div
            class="px-3 pb-1 pt-2 text-[10px] font-semibold uppercase tracking-wide text-muted-foreground"
          >
            {{ g.title }}
          </div>
          <button
            v-for="item in g.items"
            :key="item.site.name + g.title"
            type="button"
            class="flex w-full items-start gap-2 px-3 py-2 text-left transition-colors hover:bg-accent/60"
            :class="flat[selected] === item ? 'bg-accent/80' : ''"
            @click="openSite(item.site)"
            @mouseenter="selected = flat.indexOf(item)"
          >
            <div class="min-w-0 flex-1">
              <div class="truncate font-mono text-sm font-medium">{{ item.label }}</div>
              <div class="truncate font-mono text-[11px] text-muted-foreground">
                {{ item.sublabel }}
              </div>
            </div>
            <div class="flex shrink-0 items-center gap-0.5" @click.stop>
              <Button
                variant="ghost"
                size="icon"
                class="size-7"
                title="Favorite"
                @click="toggleFav(item.site)"
              >
                <Star
                  class="size-3.5"
                  :class="
                    prefs.favorites.includes(item.site.name)
                      ? 'fill-amber-400 text-amber-400'
                      : ''
                  "
                />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="size-7"
                title="Open in browser"
                @click="openSite(item.site)"
              >
                <ExternalLink class="size-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="size-7"
                title="Open in IDE"
                :disabled="busy === `ide:${item.site.name}`"
                @click="openIde(item.site)"
              >
                <SquareTerminal class="size-3.5" />
              </Button>
              <DropdownMenu>
                <DropdownMenuTrigger as-child>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="size-7"
                    :aria-label="`More actions for ${item.site.name}`"
                    title="More actions"
                  >
                    <MoreHorizontal class="size-3.5" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" class="min-w-44">
                  <DropdownMenuItem
                    :disabled="busy === `term:${item.site.name}`"
                    @select="void openTerm(item.site)"
                  >
                    <Terminal class="size-4" />
                    Terminal
                  </DropdownMenuItem>
                  <DropdownMenuItem @select="void reveal(item.site)">
                    <FolderOpen class="size-4" />
                    Reveal folder
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    :disabled="busy === `php:${item.site.name}`"
                    @select="void restartPool(item.site)"
                  >
                    <RotateCw
                      class="size-4"
                      :class="busy === `php:${item.site.name}` ? 'animate-spin' : ''"
                    />
                    Restart PHP {{ item.site.php }} pool
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </button>
        </template>
      </div>
    </div>

    <div v-if="serviceRows.length" class="shrink-0 border-t">
      <button
        type="button"
        class="flex w-full items-center gap-1.5 px-3 py-2 text-left hover:bg-accent/40"
        :aria-expanded="servicesOpen"
        @click="servicesOpen = !servicesOpen"
      >
        <component
          :is="servicesOpen ? ChevronDown : ChevronRight"
          class="size-3.5 shrink-0 text-muted-foreground"
        />
        <span class="text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
          Services
        </span>
        <span class="ml-auto font-mono text-[10px] text-muted-foreground">
          {{ serviceRows.length }}
        </span>
      </button>
      <div v-if="servicesOpen" class="max-h-36 overflow-y-auto px-3 pb-2">
        <div class="space-y-0.5">
          <div
            v-for="row in serviceRows"
            :key="row.id"
            class="flex items-center gap-2 rounded-md px-1 py-1 hover:bg-accent/40"
            :class="isRowBusy(row.id) ? 'bg-accent/50' : ''"
          >
            <span
              class="size-1.5 shrink-0 rounded-full"
              :class="{
                'bg-emerald-500': row.health === 'ok',
                'bg-amber-500': row.health === 'warn',
                'bg-red-500': row.health === 'bad',
              }"
            />
            <div class="min-w-0 flex-1">
              <div class="truncate text-xs font-medium">{{ row.label }}</div>
              <div class="truncate text-[10px] text-muted-foreground">
                {{ poolStateLabel(row.state) }}
              </div>
            </div>
            <div class="flex shrink-0 items-center gap-0.5" @click.stop>
              <Button
                v-if="row.canStart"
                variant="ghost"
                size="icon"
                class="size-7"
                title="Start"
                :disabled="isRowBusy(row.id)"
                @click.stop="void doStart(row)"
              >
                <Spinner v-if="rowBusyAction(row.id) === 'start'" class="size-3.5" />
                <Play v-else class="size-3.5" />
              </Button>
              <Button
                v-if="row.canStop"
                variant="ghost"
                size="icon"
                class="size-7"
                title="Stop"
                :disabled="isRowBusy(row.id)"
                @click.stop="void doStop(row)"
              >
                <Spinner v-if="rowBusyAction(row.id) === 'stop'" class="size-3.5" />
                <Square v-else class="size-3.5" />
              </Button>
              <Button
                v-if="row.canRestart"
                variant="ghost"
                size="icon"
                class="size-7"
                title="Restart"
                :disabled="isRowBusy(row.id)"
                @click.stop="void doRestart(row)"
              >
                <RotateCw
                  class="size-3.5"
                  :class="rowBusyAction(row.id) === 'restart' ? 'animate-spin' : ''"
                />
              </Button>
            </div>
          </div>
        </div>
      </div>
    </div>

    <div v-if="showActivity" class="shrink-0 border-t">
      <button
        type="button"
        class="flex w-full items-center gap-1.5 px-3 py-2 text-left hover:bg-accent/40"
        :aria-expanded="activityOpen"
        @click="activityOpen = !activityOpen"
      >
        <component
          :is="activityOpen ? ChevronDown : ChevronRight"
          class="size-3.5 shrink-0 text-muted-foreground"
        />
        <span class="text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
          Activity
        </span>
        <span class="ml-auto font-mono text-[10px] text-muted-foreground">
          {{ activityCount }}
        </span>
      </button>
      <div v-if="activityOpen" class="max-h-40 overflow-y-auto px-3 pb-2">
        <div class="space-y-1">
          <button
            v-for="alert in alerts"
            :key="alert.id"
            type="button"
            class="flex w-full items-start gap-2 rounded-md px-1.5 py-1.5 text-left transition-colors hover:bg-accent/60"
            @click="openAlert(alert)"
          >
            <component
              :is="alertIcon(alert)"
              class="mt-0.5 size-3.5 shrink-0"
              :class="
                alert.tone === 'bad'
                  ? 'text-red-500'
                  : alert.tone === 'warn'
                    ? 'text-amber-500'
                    : 'text-muted-foreground'
              "
            />
            <div class="min-w-0 flex-1">
              <div class="truncate text-xs font-medium">{{ alert.title }}</div>
              <div v-if="alert.detail" class="truncate text-[10px] text-muted-foreground">
                {{ alert.detail }}
              </div>
            </div>
            <span class="shrink-0 text-[10px] text-brand">{{ alert.actionLabel }}</span>
          </button>

          <button
            v-for="m in recentMail"
            :key="m.id"
            type="button"
            class="flex w-full items-start gap-2 rounded-md px-1.5 py-1.5 text-left transition-colors hover:bg-accent/60"
            @click="openMail"
          >
            <Mail class="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-1.5">
                <span
                  v-if="m.read === false"
                  class="size-1.5 shrink-0 rounded-full bg-brand"
                  title="Unread"
                />
                <span class="truncate text-xs font-medium">{{ m.subject || "(no subject)" }}</span>
              </div>
              <div class="truncate text-[10px] text-muted-foreground">
                {{ m.from }}{{ m.date_epoch ? ` · ${formatMailTime(m.date_epoch)}` : "" }}
              </div>
            </div>
          </button>
        </div>
        <div class="mt-1.5 flex flex-col gap-1">
          <button
            type="button"
            class="flex items-center gap-1.5 text-[11px] text-brand hover:underline"
            @click="openDoctor"
          >
            <Stethoscope class="size-3 shrink-0" />
            Open Doctor…
          </button>
          <button
            type="button"
            class="flex items-center gap-1.5 text-[11px] text-brand hover:underline"
            @click="openInspect"
          >
            Open Inspect / Dumps…
          </button>
        </div>
      </div>
    </div>

    <div class="flex items-center gap-2 border-t px-3 py-2">
      <Button size="sm" variant="secondary" class="flex-1" @click="newSite"> New site… </Button>
      <Button size="sm" variant="ghost" @click="hide">Esc</Button>
    </div>

    <Modal v-model:open="stopDaemonOpen" title="Stop daemon">
      <p class="text-sm text-muted-foreground">
        This stops all <strong class="text-foreground">.{{ tld }}</strong> sites, DNS, and
        databases until you start Yerd again.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button @click="confirmStopDaemon(close)">Stop</Button>
      </template>
    </Modal>

    <Modal v-model:open="restartDaemonOpen" title="Restart daemon">
      <p class="text-sm text-muted-foreground">
        This briefly stops all <strong class="text-foreground">.{{ tld }}</strong> sites, DNS,
        and this connection while the daemon restarts. It returns in a few seconds.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button @click="confirmRestartDaemon(close)">Restart</Button>
      </template>
    </Modal>
  </div>
</template>
