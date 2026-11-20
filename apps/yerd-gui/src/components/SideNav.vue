<script setup lang="ts">
import type { Component } from "vue";
import { computed, onMounted, ref, watch } from "vue";
import {
  Activity,
  ArrowLeftRight,
  CodeXml,
  Database,
  Globe,
  Hexagon,
  Inbox,
  LayoutDashboard,
  Settings,
  Share2,
  Stethoscope,
  Wrench,
} from "lucide-vue-next";

import NavLink from "@/components/NavLink.vue";
import OperationsIndicator from "@/components/OperationsIndicator.vue";
import TitleBar from "@/components/TitleBar.vue";
import { useDaemon } from "@/composables/useDaemon";
import { loadPlatform, usePlatform } from "@/composables/usePlatform";
import { useResource } from "@/composables/useResource";
import { cachedUpdateStatus, dumpsStatus, listPhp, showMailsWindow } from "@/ipc/client";
import { needsElevation } from "@/lib/elevation";
import logoUrl from "@/assets/logo.svg";

// The main window's full-height sidebar, laid out from the "Yerd App" draft.
// Top: the window controls (TitleBar's sidebar variant, so they share its drag
// region and zoom handling) with the brand block and a live daemon status line
// in its slot. Then one flat list of views with no section headings, and
// Doctor + Settings pinned at the bottom. Icons are monochrome - see NavLink.
//
// Badges: Sites shows the parked + linked count; PHP and Node their update
// counts; Mail its unread count (clicking the pill opens the viewer); Share the
// shared-site count; Dumps an on/off pill; Doctor an amber dot when an OS
// privilege is unelevated; Settings a 1 when a Yerd update is available.
type Item = {
  to: string;
  label: string;
  icon: Component;
  badge?: number;
  onBadgeClick?: () => void;
  badgeTitle?: string;
  state?: "on" | "off";
  warn?: boolean;
  warnTitle?: string;
};

const { connected, report } = useDaemon();
const { isMac } = usePlatform();
const siteCount = computed(() =>
  report.value ? report.value.sites.parked + report.value.sites.linked : 0,
);
const unread = computed(() => report.value?.mail?.unread ?? 0);
const sharedSites = computed(() => report.value?.shared_sites ?? 0);
const nodeUpdates = computed(() => report.value?.node_updates ?? 0);
const daemonVersion = computed(() => report.value?.daemon_version ?? "");

// Same shared "php" cache the PHP view uses, so the badge count matches what
// that page shows. `updates` is populated on the list_php response.
const { data: phpData } = useResource("php", listPhp);
const phpUpdates = computed(() => phpData.value?.updates?.length ?? 0);

// Dumps interception has no field on the status report, so it is re-read on
// every status tick (the Dumps page invalidates this key when it toggles).
const { data: dumps, refresh: refreshDumps } = useResource("dumps-status", dumpsStatus, {
  immediate: false,
});
watch(
  report,
  (r) => {
    if (r) void refreshDumps();
  },
  { immediate: true },
);
const dumpsEnabled = computed<"on" | "off" | undefined>(() => {
  if (!report.value || !dumps.value) return undefined;
  return dumps.value.enabled ? "on" : "off";
});

// Yerd self-update: the daemon's last stored check (no network). Shows a 1 when
// an update is available on the current track, nothing when up to date.
const yerdUpdate = ref(0);

const unelevated = computed(() =>
  report.value ? needsElevation(report.value, isMac.value) : false,
);

onMounted(() => {
  void loadPlatform();
  cachedUpdateStatus()
    .then((s) => (yerdUpdate.value = s.available ? 1 : 0))
    .catch(() => {});
});

const items = computed<Item[]>(() => [
  { to: "/overview", label: "Overview", icon: LayoutDashboard },
  { to: "/sites", label: "Sites", icon: Globe, badge: siteCount.value },
  { to: "/proxies", label: "Proxies", icon: ArrowLeftRight },
  { to: "/php", label: "PHP", icon: CodeXml, badge: phpUpdates.value },
  { to: "/node", label: "Node", icon: Hexagon, badge: nodeUpdates.value },
  { to: "/tooling", label: "Tooling", icon: Wrench },
  { to: "/services", label: "Services", icon: Database },
  {
    to: "/mail",
    label: "Mail",
    icon: Inbox,
    badge: unread.value,
    onBadgeClick: () => void showMailsWindow(),
    badgeTitle: "Open mail viewer",
  },
  { to: "/dumps", label: "Dumps", icon: Activity, state: dumpsEnabled.value },
  { to: "/integrations", label: "Share", icon: Share2, badge: sharedSites.value },
]);

const pinned = computed<Item[]>(() => [
  {
    to: "/doctor",
    label: "Doctor",
    icon: Stethoscope,
    warn: unelevated.value,
    warnTitle: "Something needs elevated permissions",
  },
  {
    to: "/general",
    label: "Settings",
    icon: Settings,
    badge: yerdUpdate.value,
    badgeTitle: "Update available",
  },
]);
</script>

<template>
  <nav class="flex h-full w-[232px] shrink-0 flex-col border-r bg-sidebar">
    <TitleBar variant="sidebar">
      <div class="flex items-center gap-3 px-[18px] pt-1">
        <img :src="logoUrl" alt="" class="size-8 shrink-0 rounded-lg shadow-sm" />
        <div class="flex min-w-0 flex-col gap-1">
          <div class="flex min-w-0 items-center gap-2">
            <p class="font-display text-[15px] font-normal leading-none tracking-[0.6px]">YERD</p>
            <span
              v-if="connected === true && daemonVersion"
              class="truncate rounded-[4px] border px-1 py-[2px] font-mono text-[10px] leading-none text-faint"
              data-testid="daemon-version"
              >v{{ daemonVersion }}</span
            >
          </div>
          <p
            class="flex min-w-0 items-center gap-1.5 text-xs leading-none"
            :class="connected === false ? 'text-destructive' : 'text-muted-foreground'"
            data-testid="daemon-status"
          >
            <span
              class="size-1.5 shrink-0 rounded-full"
              :class="
                connected === true
                  ? 'bg-success'
                  : connected === false
                    ? 'bg-destructive'
                    : 'bg-muted-foreground'
              "
              aria-hidden="true"
            />
            <span v-if="connected === true" class="truncate">Running</span>
            <span v-else-if="connected === false" class="truncate">Unreachable</span>
            <span v-else class="truncate">Connecting…</span>
          </p>
        </div>
      </div>
    </TitleBar>

    <!-- Scrolls on very short windows; shows the app's slim scrollbar (styled in
         style.css) only when the items overflow. -->
    <div class="scrollbar-slim mt-6 flex-1 overflow-y-auto px-2.5">
      <ul class="flex flex-col gap-0.5" data-testid="nav-main">
        <li v-for="item in items" :key="item.to">
          <NavLink v-bind="item" />
        </li>
      </ul>
    </div>

    <div class="px-2.5 pb-3 pt-2">
      <OperationsIndicator class="px-2.5" />
      <ul class="flex flex-col gap-0.5" data-testid="nav-pinned">
        <li v-for="item in pinned" :key="item.to">
          <NavLink v-bind="item" />
        </li>
      </ul>
    </div>
  </nav>
</template>
