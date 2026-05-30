<script setup lang="ts">
import { onMounted, onUnmounted } from "vue";

import AppShell from "@/components/AppShell.vue";
import Toaster from "@/components/ui/Toaster.vue";
import { useDaemon } from "@/composables/useDaemon";

// Start the single shared daemon poller for the app's lifetime.
const { start, stop } = useDaemon();
onMounted(() => start(4000));
onUnmounted(() => stop());
</script>

<template>
  <AppShell />
  <Toaster />
</template>
