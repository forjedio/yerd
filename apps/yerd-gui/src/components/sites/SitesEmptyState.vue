<script setup lang="ts">
import { ChevronRight, FolderPlus, FolderOpen, Globe, Link2, Sparkles } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";

/**
 * The Sites screen with nothing parked, linked or grouped: park a folder of
 * projects, start a new project, or link a single existing one.
 */
defineProps<{ tld: string; parking?: boolean }>();

const emit = defineEmits<{ park: []; create: []; link: [] }>();
</script>

<template>
  <div class="flex h-full flex-col items-center justify-center px-6 py-12 text-center" data-testid="sites-empty">
    <span class="flex size-14 items-center justify-center rounded-2xl border border-brand/40 bg-brand/10 text-brand">
      <Globe class="size-6" aria-hidden="true" />
    </span>
    <h2 class="mt-5 text-[22px] font-semibold tracking-[-0.2px]">Serve your first site</h2>
    <p class="mt-2 text-sm text-muted-foreground">
      Park a folder and every project inside it is live on .{{ tld }}, no config.
    </p>

    <div class="mt-9 grid w-full max-w-[760px] gap-4 text-left sm:grid-cols-[1fr_1.05fr]">
      <div
        class="flex flex-col items-center justify-center rounded-xl border border-dashed border-brand/60 bg-brand/5 px-6 py-7 text-center"
      >
        <FolderPlus class="size-5 text-brand" aria-hidden="true" />
        <p class="mt-3 text-[15px] font-semibold">Choose a folder of projects</p>
        <p class="mt-1 font-mono text-xs text-muted-foreground">e.g. ~/Code or ~/Sites</p>
        <Button size="sm" class="mt-4" :disabled="parking" @click="emit('park')">
          <FolderOpen /> Choose folder…
        </Button>
      </div>
      <div class="flex flex-col gap-3">
        <button
          type="button"
          class="flex flex-1 items-center gap-3.5 rounded-xl border bg-card px-4 py-4 text-left transition-colors hover:border-brand/40 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          @click="emit('create')"
        >
          <Sparkles class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
          <span class="min-w-0 flex-1">
            <span class="block text-sm font-semibold">Start a new project</span>
            <span class="block text-xs text-muted-foreground">Laravel, WordPress, Slim or another starter</span>
          </span>
          <ChevronRight class="size-4 shrink-0 text-faint" aria-hidden="true" />
        </button>
        <button
          type="button"
          class="flex flex-1 items-center gap-3.5 rounded-xl border bg-card px-4 py-4 text-left transition-colors hover:border-brand/40 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          @click="emit('link')"
        >
          <Link2 class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
          <span class="min-w-0 flex-1">
            <span class="block text-sm font-semibold">Link one existing project</span>
            <span class="block text-xs text-muted-foreground">Serve a single folder from anywhere on disk</span>
          </span>
          <ChevronRight class="size-4 shrink-0 text-faint" aria-hidden="true" />
        </button>
      </div>
    </div>
  </div>
</template>
