<script setup lang="ts">
import { ArrowLeftRight, ArrowRight, CornerDownRight, Globe, Plus } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";

/**
 * The Proxies screen with no proxies and no path rules: two example tiles that
 * open New proxy in their mode, and the New proxy button. The path tile needs a
 * site to attach to.
 */
defineProps<{ tld: string; hasSites: boolean }>();

const emit = defineEmits<{ create: [mode?: "domain" | "path"] }>();

const TILE =
  "flex flex-col gap-1.5 rounded-xl border bg-card px-4 py-3.5 text-left transition-colors hover:border-brand/40 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:border-border";
</script>

<template>
  <div class="flex h-full flex-col items-center justify-center px-6 py-12 text-center" data-testid="proxies-empty">
    <span class="flex size-[52px] items-center justify-center rounded-xl border border-brand/40 bg-brand/10 text-brand">
      <ArrowLeftRight class="size-5" aria-hidden="true" />
    </span>
    <h2 class="mt-5 text-[22px] font-semibold tracking-[-0.2px]">No proxies yet</h2>
    <p class="mt-2 max-w-[380px] text-sm text-muted-foreground">
      Give anything on localhost a .{{ tld }} name — or hand one path of a site to another app.
    </p>

    <div class="mt-7 grid w-full max-w-[564px] gap-4 sm:grid-cols-2">
      <button type="button" :class="TILE" data-testid="empty-domain" @click="emit('create', 'domain')">
        <span class="flex items-center gap-2 text-[13px] font-semibold">
          <Globe class="size-3.5 text-brand" aria-hidden="true" /> A whole domain
        </span>
        <span class="flex items-center gap-1 font-mono text-xs text-muted-foreground">
          grafana.{{ tld }} <ArrowRight class="size-3" aria-hidden="true" /> localhost:3000
        </span>
        <span class="text-xs text-faint">Docker containers, Node apps, Vite</span>
      </button>
      <button
        type="button"
        :class="TILE"
        :disabled="!hasSites"
        :title="hasSites ? undefined : 'Add a site first'"
        data-testid="empty-path"
        @click="emit('create', 'path')"
      >
        <span class="flex items-center gap-2 text-[13px] font-semibold">
          <CornerDownRight class="size-3.5 text-brand" aria-hidden="true" /> A path on a site
        </span>
        <span class="flex items-center gap-1 font-mono text-xs text-muted-foreground">
          codestash.{{ tld }}/live <ArrowRight class="size-3" aria-hidden="true" /> :8081
        </span>
        <span class="text-xs text-faint">Websockets, a SPA, an API beside PHP</span>
      </button>
    </div>

    <Button class="mt-5" @click="emit('create')"><Plus /> New proxy</Button>
  </div>
</template>
