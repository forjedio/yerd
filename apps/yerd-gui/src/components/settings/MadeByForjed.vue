<script setup lang="ts">
import { openInBrowser } from "@/ipc/client";

/** The "Made by Forjed" credit at the foot of Settings, linking to forjed.io. */
</script>

<template>
  <p class="text-[12px] text-faint">
    Made by
    <button
      type="button"
      class="ml-1 font-semibold text-brand hover:underline"
      @click="openInBrowser('https://forjed.io')"
    >
      Forjed
    </button>
  </p>
</template>
