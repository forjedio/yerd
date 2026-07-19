<script setup lang="ts">
import type { Component } from "vue";
import { computed, onMounted, ref } from "vue";
import {
  ClipboardList,
  Database,
  Info,
  LayoutDashboard,
  LayoutGrid,
  Mail,
  Settings,
  Share2,
  SquareCode,
  Stethoscope,
  Waypoints,
  Wrench,
} from "lucide-vue-next";

import NavLink from "@/components/NavLink.vue";
import OperationsIndicator from "@/components/OperationsIndicator.vue";
import StatusPill from "@/components/StatusPill.vue";
import { useDaemon } from "@/composables/useDaemon";
import { loadPlatform, usePlatform } from "@/composables/usePlatform";
import { useResource } from "@/composables/useResource";
import { cachedUpdateStatus, listPhp, showMailsWindow } from "@/ipc/client";
import { needsElevation } from "@/lib/elevation";
import logoUrl from "@/assets/logo.svg";

// Grouped left nav. Sections name the app's concerns: the runtime you configure
// (Environment: sites, PHP, services), the developer tooling around it
// (Developer), and the system itself (System). Overview sits above them as the
// home/dashboard. Icons are monochrome - see NavLink; status colour is reserved
// for the pill below. The Mail item carries an optional unread-count badge whose
// click opens the viewer. PHP/About carry passive update-count badges; Doctor
// carries an amber warn marker when an OS privilege is unelevated.
type Item = {
  to: string;
  label: string;
  icon: Component;
  badge?: number;
  onBadgeClick?: () => void;
  badgeTitle?: string;
  warn?: boolean;
  warnTitle?: string;
};

const { connected, report } = useDaemon();
const { isMac } = usePlatform();
const unread = computed(() => report.value?.mail?.unread ?? 0);
const sharedSites = computed(() => report.value?.shared_sites ?? 0);

// Same shared "php" cache the PHP view uses, so the badge count matches what
// that page shows. `updates` is populated on the list_php response.
const { data: phpData } = useResource("php", listPhp);
const phpUpdates = computed(() => phpData.value?.updates?.length ?? 0);

// Yerd self-update: the daemon's last stored check (no network). Shows a 1 when
// an update is available on the current track, nothing when up to date.
const yerdUpdate = ref(0);

// Unelevated OS privileges (CA trust, .test resolver, ports) → amber ! on Doctor.
const unelevated = computed(() =>
  report.value ? needsElevation(report.value, isMac.value) : false,
);

onMounted(() => {
  void loadPlatform();
  cachedUpdateStatus()
    .then((s) => (yerdUpdate.value = s.available ? 1 : 0))
    .catch(() => {});
});

// A computed (not a const) so the Mail item's unread badge stays reactive.
const sections = computed<{ title: string; items: Item[] }[]>(() => [
  {
    title: "General",
    items: [
      { to: "/overview", label: "Overview", icon: LayoutDashboard },
      { to: "/about", label: "About", icon: Info, badge: yerdUpdate.value },
    ],
  },
  {
    title: "Environment",
    items: [
      { to: "/sites", label: "Sites", icon: LayoutGrid },
      { to: "/php", label: "PHP", icon: SquareCode, badge: phpUpdates.value },
      { to: "/services", label: "Services", icon: Database },
    ],
  },
  {
    title: "Developer",
    items: [
      { to: "/tooling", label: "Tooling", icon: Wrench },
      { to: "/proxies", label: "Proxies", icon: Waypoints },
      {
        to: "/mail",
        label: "Mail",
        icon: Mail,
        badge: unread.value,
        onBadgeClick: () => void showMailsWindow(),
        badgeTitle: "Open mail viewer",
      },
      { to: "/dumps", label: "Dumps", icon: ClipboardList },
    ],
  },
  {
    title: "Integrations",
    items: [
      {
        to: "/integrations",
        label: "Share",
        icon: Share2,
        badge: sharedSites.value,
      },
    ],
  },
  {
    title: "System",
    items: [
      { to: "/general", label: "Settings", icon: Settings },
      {
        to: "/doctor",
        label: "Doctor",
        icon: Stethoscope,
        warn: unelevated.value,
        warnTitle: "Something needs elevated permissions",
      },
    ],
  },
]);
</script>

<template>
  <nav
    class="flex h-full w-56 shrink-0 flex-col border-r bg-muted px-3 py-3 dark:bg-card/40"
  >
    <!-- Brand lockup - the logo's indigo is the app's one accent. -->
    <div class="mb-6 flex items-center gap-2.5 px-2 pt-1">
      <img :src="logoUrl" alt="" class="size-6 rounded-[7px]" />
      <span
        class="relative top-[3px] font-display text-lg font-normal leading-none tracking-wide"
        >YERD</span
      >
    </div>

    <!-- Scrolls on very short windows; shows the app's slim scrollbar (styled in
         style.css) only when the items overflow. -->
    <div class="scrollbar-slim flex flex-1 flex-col gap-5 overflow-y-auto">
      <div v-for="section in sections" :key="section.title">
        <p
          class="mb-1 px-2 font-display text-xs font-normal uppercase tracking-wider text-muted-foreground/70"
        >
          {{ section.title }}
        </p>
        <ul class="flex flex-col gap-0.5">
          <li v-for="item in section.items" :key="item.to">
            <NavLink v-bind="item" />
          </li>
        </ul>
      </div>
    </div>

    <div class="mt-2 border-t px-2 pt-3">
      <OperationsIndicator />
      <StatusPill
        v-if="connected === true"
        tone="ok"
        label="Daemon connected"
        pulse
      />
      <StatusPill
        v-else-if="connected === false"
        tone="bad"
        label="Daemon unreachable"
      />
      <StatusPill v-else tone="unknown" label="Connecting…" />
    </div>
  </nav>
</template>
