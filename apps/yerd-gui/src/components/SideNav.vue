<script setup lang="ts">
import {
  ClipboardList,
  Info,
  LayoutGrid,
  Mail,
  Server,
  Settings,
  SquareCode,
  Stethoscope,
} from "lucide-vue-next";
import { RouterLink } from "vue-router";

import StatusPill from "@/components/StatusPill.vue";
import { useDaemon } from "@/composables/useDaemon";

// Left-hand nav, Herd-style. Icon-chip colours are grouped by role and reused:
// grey for configuration (General, PHP, Sites), red for Services, orange for
// Dumps, blue for the info pages (Doctor, About). The chip keeps its colour even
// when the item is active — only the row background highlights the selection.
const GREY = "bg-zinc-500/15 text-zinc-600 dark:text-zinc-400";
const RED = "bg-red-500/15 text-red-600 dark:text-red-400";
const ORANGE = "bg-orange-500/15 text-orange-600 dark:text-orange-400";
const BLUE = "bg-blue-500/15 text-blue-600 dark:text-blue-400";

const items = [
  { to: "/general", label: "General", icon: Settings, chip: GREY },
  { to: "/php", label: "PHP", icon: SquareCode, chip: GREY },
  { to: "/sites", label: "Sites", icon: LayoutGrid, chip: GREY },
  { to: "/services", label: "Services", icon: Server, chip: RED },
  { to: "/dumps", label: "Dumps", icon: ClipboardList, chip: ORANGE },
  { to: "/mail", label: "Mail", icon: Mail, chip: RED },
  { to: "/doctor", label: "Doctor", icon: Stethoscope, chip: BLUE },
  { to: "/about", label: "About", icon: Info, chip: BLUE },
];

const { connected } = useDaemon();
</script>

<template>
  <nav
    class="flex h-full w-52 shrink-0 flex-col border-r bg-muted px-3 py-4 dark:bg-card/40"
  >
    <ul class="flex flex-1 flex-col gap-1">
      <li v-for="item in items" :key="item.to">
        <RouterLink :to="item.to" custom v-slot="{ isActive, href, navigate }">
          <a
            :href="href"
            :aria-current="isActive ? 'page' : undefined"
            class="group flex items-center gap-2.5 rounded-lg px-2 py-1.5 text-sm font-medium transition-colors"
            :class="
              isActive
                ? 'bg-blue-500/10 text-blue-700 dark:bg-blue-400/40 dark:text-blue-50'
                : 'text-muted-foreground hover:bg-accent/70 hover:text-foreground'
            "
            @click="navigate"
          >
            <span
              class="flex size-6 shrink-0 items-center justify-center rounded-md transition-colors"
              :class="item.chip"
            >
              <component :is="item.icon" class="size-4" />
            </span>
            {{ item.label }}
          </a>
        </RouterLink>
      </li>
    </ul>

    <div class="mt-auto border-t px-2 pt-3">
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
