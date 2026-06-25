<script setup lang="ts">
import { computed } from "vue";
import { useRoute } from "vue-router";

import DaemonDownHero from "@/components/DaemonDownHero.vue";
import PageHeader from "@/components/PageHeader.vue";
import SideNav from "@/components/SideNav.vue";
import TitleBar from "@/components/TitleBar.vue";
import { useDaemon } from "@/composables/useDaemon";

const route = useRoute();
const { unreachable } = useDaemon();

// Only three views work without a live daemon: Overview (owns its own
// daemon-down hero + start affordance), Settings/General (can start/install it),
// and About (pure build info). Every data-backed view — PHP, Sites, Tooling,
// Services, Dumps, Mail, Doctor — is blocked by the daemon-down screen so none of
// them mount, fire a doomed request, and strand the user on a blank table.
const DAEMON_FREE = new Set(["overview", "general", "about"]);
const showPanel = computed(
  () => unreachable.value && !DAEMON_FREE.has(String(route.name)),
);

// Mirror the blocked route's own header on the daemon-down screen.
const pageTitle = computed(() => String(route.meta.title ?? "Yerd"));
const pageSubtitle = computed(() =>
  typeof route.meta.subtitle === "string" ? route.meta.subtitle : undefined,
);
</script>

<template>
  <div class="flex h-full w-full flex-col overflow-hidden">
    <TitleBar />

    <div class="flex min-h-0 flex-1 overflow-hidden">
      <SideNav />

      <!-- Clip, don't scroll: every routed view is `h-full` and owns its own
           inner `overflow-y-auto`, so a scroll container here would be a second,
           redundant scrollbar nested inside the view's own. -->
      <main class="min-w-0 flex-1 overflow-hidden">
        <!-- Daemon-dependent routes show the shared daemon-down screen when the
             socket is unreachable: the page's own header (no action buttons) plus
             the same hero the Overview uses. Overview / Settings / About are
             exempt (see DAEMON_FREE) — they start/install it or degrade. -->
        <div v-if="showPanel" class="flex h-full flex-col">
          <PageHeader :title="pageTitle" :subtitle="pageSubtitle" />
          <div class="flex-1 space-y-4 overflow-y-auto p-6">
            <DaemonDownHero />
          </div>
        </div>

        <RouterView v-else />
      </main>
    </div>
  </div>
</template>
