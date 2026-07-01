<script setup lang="ts">
import { ArrowUpRight } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import { openInBrowser } from "@/ipc/client";

// `docs` is a path under the documentation site (e.g. "/guide/mail"); when set,
// a Docs button opens that page in the user's browser.
const props = defineProps<{ title: string; subtitle?: string; docs?: string }>();

function openDocs(): void {
  if (props.docs) void openInBrowser(`https://yerd.app${props.docs}`);
}
</script>

<template>
  <header
    class="sticky top-0 z-10 flex items-center justify-between gap-4 border-b bg-muted px-6 py-4 dark:bg-card/40"
  >
    <div>
      <h2 class="font-display text-xl font-normal tracking-wide">{{ title }}</h2>
      <p v-if="subtitle" class="text-sm text-muted-foreground">{{ subtitle }}</p>
    </div>
    <div class="flex items-center gap-2">
      <Button v-if="docs" variant="outline" @click="openDocs">
        Docs
        <ArrowUpRight class="opacity-70" />
      </Button>
      <slot name="actions" />
    </div>
  </header>
</template>
