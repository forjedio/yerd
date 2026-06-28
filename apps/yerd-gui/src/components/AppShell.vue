<script setup lang="ts">
import { computed } from "vue";
import { useRoute } from "vue-router";

import CommandPalette from "@/components/CommandPalette.vue";
import DaemonDownHero from "@/components/DaemonDownHero.vue";
import PageHeader from "@/components/PageHeader.vue";
import ShortcutsCheatSheet from "@/components/ShortcutsCheatSheet.vue";
import SideNav from "@/components/SideNav.vue";
import TitleBar from "@/components/TitleBar.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useShortcuts } from "@/lib/shortcuts/useShortcuts";
import { useSiteCommands } from "@/lib/shortcuts/useSiteCommands";

const route = useRoute();
const { unreachable } = useDaemon();
const { paletteOpen, cheatSheetOpen, commands, run } = useShortcuts("main");
const siteCommands = useSiteCommands(paletteOpen);
const paletteCommands = computed(() => [...commands, ...siteCommands.value]);

// Only three views work without a live daemon: Overview (owns its own
// daemon-down hero + start affordance), Settings/General (can start/install it),
// and About (pure build info). Every data-backed view - PHP, Sites, Tooling,
// Services, Dumps, Mail, Doctor - is blocked by the daemon-down screen so none of
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
           redundant scrollbar nested inside the view's own.
           `relative` is load-bearing, not cosmetic: it makes `<main>` the
           containing block for any absolutely-positioned descendant (e.g. the
           Doctor page's `<thead class="sr-only">` a11y header). Without a
           positioned ancestor, such an element's containing block is the
           viewport, so it escapes this `overflow-hidden` clip, lands at its
           static-flow Y deep in a scrolled list, and inflates the *document's*
           scroll height - producing a second, window-level scrollbar that
           reveals the desktop behind the transparent window. Pinning it here
           keeps every stray abspos clipped to the content region. -->
      <main class="relative min-w-0 flex-1 overflow-hidden">
        <!-- Daemon-dependent routes show the shared daemon-down screen when the
             socket is unreachable: the page's own header (no action buttons) plus
             the same hero the Overview uses. Overview / Settings / About are
             exempt (see DAEMON_FREE) - they start/install it or degrade. -->
        <div v-if="showPanel" class="flex h-full flex-col">
          <PageHeader :title="pageTitle" :subtitle="pageSubtitle" />
          <div class="flex-1 space-y-4 overflow-y-auto p-6">
            <DaemonDownHero />
          </div>
        </div>

        <RouterView v-else />
      </main>
    </div>

    <CommandPalette
      v-model:open="paletteOpen"
      :commands="paletteCommands"
      :run="run"
    />
    <ShortcutsCheatSheet v-model:open="cheatSheetOpen" :commands="commands" />
  </div>
</template>
