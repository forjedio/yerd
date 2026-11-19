<script setup lang="ts">
import { Plus } from "lucide-vue-next";
import { computed, nextTick, ref } from "vue";

import SiteDomainsPanel from "@/components/SiteDomainsPanel.vue";
import Modal from "@/components/ui/Modal.vue";
import type { SiteEntry } from "@/ipc/types";

/**
 * The inspector's DOMAINS card: a read view of every domain the site answers on,
 * tagged primary or wildcard. Any row, or Add domain, opens the shared
 * `SiteDomainsPanel` in a dialog, where adding, removing, making primary and
 * resetting all live unchanged.
 */
const props = defineProps<{
  site: SiteEntry;
  tld: string;
}>();

const emit = defineEmits<{ domainsChanged: [] }>();

const primary = computed(() => props.site.primary_domain ?? `${props.site.name}.${props.tld}`);

/** Mirrors `SiteDomainsPanel.effective`: the DTO omits `domains` for a default site. */
const effective = computed(() =>
  props.site.domains && props.site.domains.length ? props.site.domains : [primary.value],
);

const open = ref(false);

function openPanel(): void {
  void nextTick(() => {
    open.value = true;
  });
}
</script>

<template>
  <section>
    <h3 class="mb-2 px-1 text-[11px] font-semibold uppercase tracking-[0.6px] text-faint">
      Domains
    </h3>
    <div class="divide-y overflow-hidden rounded-[10px] border bg-card" data-testid="domains-card">
      <button
        v-for="d in effective"
        :key="d"
        type="button"
        class="flex h-11 w-full items-center justify-between gap-3 px-4 text-left transition-colors hover:bg-accent/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
        :title="`Manage domains for ${primary}`"
        @click="openPanel"
      >
        <span class="min-w-0 truncate font-mono text-[13px]">
          <template v-if="d.startsWith('*.')"><span class="text-faint">*</span>{{ d.slice(1) }}</template>
          <template v-else>{{ d }}</template>
        </span>
        <span v-if="d === primary" class="shrink-0 text-[11px] font-medium text-brand">primary</span>
        <span v-else-if="d.startsWith('*.')" class="shrink-0 text-[11px] text-faint">wildcard</span>
      </button>
      <p
        v-if="site.apex_shadowed_by"
        class="px-4 py-2.5 text-xs text-warning"
        data-testid="domains-shadowed"
      >
        {{ site.name }}.{{ tld }} is served by “{{ site.apex_shadowed_by }}”
      </p>
      <button
        type="button"
        class="flex h-11 w-full items-center gap-2 px-4 text-left text-[13px] font-medium text-brand transition-colors hover:bg-accent/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
        @click="openPanel"
      >
        <Plus class="size-3.5" aria-hidden="true" /> Add domain
      </button>
    </div>

    <Modal v-model:open="open" :title="`Domains · ${primary}`" size="lg">
      <SiteDomainsPanel :site="site" :tld="tld" @changed="emit('domainsChanged')" />
    </Modal>
  </section>
</template>
