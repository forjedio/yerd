<script setup lang="ts">
import { PlugZap, RefreshCw } from "lucide-vue-next";

import SideNav from "@/components/SideNav.vue";
import Button from "@/components/ui/Button.vue";
import { useDaemon } from "@/composables/useDaemon";

const { unreachable, refresh } = useDaemon();
</script>

<template>
  <div class="flex h-full w-full overflow-hidden">
    <SideNav />

    <main class="flex-1 overflow-y-auto">
      <!-- Global unreachable state: the GUI is a client; it does not own the
           daemon lifecycle, so we surface the gap and offer a retry. -->
      <div
        v-if="unreachable"
        class="flex h-full flex-col items-center justify-center gap-4 p-8 text-center"
      >
        <PlugZap class="size-10 text-muted-foreground" />
        <div>
          <h2 class="text-lg font-semibold">Daemon not running</h2>
          <p class="mt-1 max-w-sm text-sm text-muted-foreground">
            Yerd can't reach the <code>yerdd</code> daemon socket. Start it with
            <code class="rounded bg-muted px-1">yerdd</code>, then retry.
          </p>
        </div>
        <Button variant="outline" @click="refresh">
          <RefreshCw class="size-4" /> Retry
        </Button>
      </div>

      <RouterView v-else />
    </main>
  </div>
</template>
