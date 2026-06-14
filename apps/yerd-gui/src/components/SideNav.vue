<script setup lang="ts">
import {
  ClipboardList,
  Flame,
  Info,
  LayoutGrid,
  Server,
  Settings,
  SquareCode,
  Stethoscope,
  type LucideIcon,
} from "lucide-vue-next";
import { RouterLink } from "vue-router";

import StatusPill from "@/components/StatusPill.vue";
import { useDaemon } from "@/composables/useDaemon";

// Left-hand nav, Herd-style. Icon-chip colours are grouped by role and reused:
// grey for configuration (General, PHP, Sites), red for Services, orange for the
// Laravel group, blue for the info pages (Doctor, About). The chip keeps its
// colour even when the item is active — only the row background highlights.
const GREY = "bg-zinc-500/15 text-zinc-600 dark:text-zinc-400";
const RED = "bg-red-500/15 text-red-600 dark:text-red-400";
const ORANGE = "bg-orange-500/15 text-orange-600 dark:text-orange-400";
const BLUE = "bg-blue-500/15 text-blue-600 dark:text-blue-400";

interface NavLink {
  to: string;
  label: string;
  icon: LucideIcon;
  chip: string;
}
interface NavGroup {
  label: string;
  icon: LucideIcon;
  chip: string;
  children: NavLink[];
}
type NavItem = NavLink | NavGroup;

const items: NavItem[] = [
  { to: "/general", label: "General", icon: Settings, chip: GREY },
  { to: "/php", label: "PHP", icon: SquareCode, chip: GREY },
  { to: "/sites", label: "Sites", icon: LayoutGrid, chip: GREY },
  { to: "/services", label: "Services", icon: Server, chip: RED },
  {
    label: "Laravel",
    icon: Flame,
    chip: ORANGE,
    children: [{ to: "/laravel/dumps", label: "Dumps", icon: ClipboardList, chip: ORANGE }],
  },
  { to: "/doctor", label: "Doctor", icon: Stethoscope, chip: BLUE },
  { to: "/about", label: "About", icon: Info, chip: BLUE },
];

function isGroup(item: NavItem): item is NavGroup {
  return "children" in item;
}

const { connected } = useDaemon();
</script>

<template>
  <nav
    class="flex h-full w-52 shrink-0 flex-col border-r bg-muted px-3 py-4 dark:bg-card/40"
  >
    <ul class="flex flex-1 flex-col gap-1">
      <template v-for="item in items" :key="item.label">
        <!-- Group: a non-clickable header with indented child links. -->
        <li v-if="isGroup(item)">
          <div
            class="flex items-center gap-2.5 rounded-lg px-2 py-1.5 text-sm font-medium text-muted-foreground"
          >
            <span
              class="flex size-6 shrink-0 items-center justify-center rounded-md"
              :class="item.chip"
            >
              <component :is="item.icon" class="size-4" />
            </span>
            {{ item.label }}
          </div>
          <ul class="ml-3 mt-0.5 flex flex-col gap-1 border-l pl-2">
            <li v-for="child in item.children" :key="child.to">
              <RouterLink :to="child.to" custom v-slot="{ isActive, href, navigate }">
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
                    :class="child.chip"
                  >
                    <component :is="child.icon" class="size-4" />
                  </span>
                  {{ child.label }}
                </a>
              </RouterLink>
            </li>
          </ul>
        </li>

        <!-- Flat link. -->
        <li v-else>
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
      </template>
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
