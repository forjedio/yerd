<script setup lang="ts">
import {
  Info,
  LayoutGrid,
  Server,
  SquareCode,
} from "lucide-vue-next";
import { RouterLink } from "vue-router";

import StatusPill from "@/components/StatusPill.vue";
import { useDaemon } from "@/composables/useDaemon";
import logoUrl from "@/assets/logo.svg";

// Left-hand nav, Herd-style. Ordered to grow: PHP, Sites, Services, About.
const items = [
  { to: "/php", label: "PHP", icon: SquareCode },
  { to: "/sites", label: "Sites", icon: LayoutGrid },
  { to: "/services", label: "Services", icon: Server },
  { to: "/about", label: "About", icon: Info },
];

const { connected } = useDaemon();
</script>

<template>
  <nav
    class="flex h-full w-52 shrink-0 flex-col border-r bg-card/40 px-3 py-4"
  >
    <div class="flex items-center gap-2.5 px-2 pb-5 pt-1">
      <img :src="logoUrl" alt="Yerd" class="size-8 rounded-lg" />
      <div class="leading-tight">
        <h1 class="text-base font-bold tracking-tight">Yerd</h1>
        <p class="text-[11px] text-muted-foreground">Local PHP dev</p>
      </div>
    </div>

    <ul class="flex flex-1 flex-col gap-1">
      <li v-for="item in items" :key="item.to">
        <RouterLink
          :to="item.to"
          class="flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground"
          active-class="bg-accent text-accent-foreground"
        >
          <component :is="item.icon" class="size-4" />
          {{ item.label }}
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
