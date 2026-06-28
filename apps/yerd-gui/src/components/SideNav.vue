<script setup lang="ts">
import type { Component } from "vue";
import {
  ClipboardList,
  Database,
  Info,
  LayoutDashboard,
  LayoutGrid,
  Mail,
  Settings,
  SquareCode,
  Stethoscope,
  Wrench,
} from "lucide-vue-next";

import NavLink from "@/components/NavLink.vue";
import OperationsIndicator from "@/components/OperationsIndicator.vue";
import StatusPill from "@/components/StatusPill.vue";
import { useDaemon } from "@/composables/useDaemon";
import logoUrl from "@/assets/logo.svg";

// Grouped left nav. Sections name the app's three real concerns: the runtime you
// configure (Environment), what the daemon supervises (Services), and the system
// itself (System). Overview sits above them as the home/dashboard. Icons are
// monochrome - see NavLink; status colour is reserved for the pill below.
type Item = { to: string; label: string; icon: Component };

const overview: Item = {
  to: "/overview",
  label: "Overview",
  icon: LayoutDashboard,
};

const sections: { title: string; items: Item[] }[] = [
  {
    title: "Environment",
    items: [
      { to: "/php", label: "PHP", icon: SquareCode },
      { to: "/sites", label: "Sites", icon: LayoutGrid },
    ],
  },
  {
    title: "Developer",
    items: [
      { to: "/tooling", label: "Tooling", icon: Wrench },
      { to: "/services", label: "Services", icon: Database },
      { to: "/mail", label: "Mail", icon: Mail },
      { to: "/dumps", label: "Dumps", icon: ClipboardList },
    ],
  },
  {
    title: "System",
    items: [
      { to: "/general", label: "Settings", icon: Settings },
      { to: "/doctor", label: "Doctor", icon: Stethoscope },
      { to: "/about", label: "About", icon: Info },
    ],
  },
];

const { connected } = useDaemon();
</script>

<template>
  <nav
    class="flex h-full w-56 shrink-0 flex-col border-r bg-muted px-3 py-3 dark:bg-card/40"
  >
    <!-- Brand lockup - the logo's indigo is the app's one accent. -->
    <div class="mb-4 flex items-center gap-2 px-2 pt-1">
      <img :src="logoUrl" alt="" class="size-6 rounded-[7px]" />
      <span class="text-sm font-semibold tracking-tight">Yerd</span>
    </div>

    <!-- Scrolls on very short windows, but its scrollbar chrome is hidden so it
         never reads as a second scrollbar beside the main content's. -->
    <div
      class="flex flex-1 flex-col gap-5 overflow-y-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
    >
      <ul>
        <li><NavLink v-bind="overview" /></li>
      </ul>

      <div v-for="section in sections" :key="section.title">
        <p
          class="mb-1 px-2 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground/70"
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
