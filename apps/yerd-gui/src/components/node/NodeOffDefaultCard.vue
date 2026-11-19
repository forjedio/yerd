<script setup lang="ts">
import Spinner from "@/components/ui/Spinner.vue";
import type { OffDefaultSite } from "@/lib/node";
import { cn } from "@/lib/utils";

/**
 * "Sites not on the default": every site whose Node resolves to something
 * other than the global default, with where that came from. Sites that
 * resolve to nothing are listed too, with a red chip.
 */
defineProps<{
  sites: OffDefaultSite[];
  loading: boolean;
  error: string | null;
}>();

const CHIP = "inline-flex h-[19px] shrink-0 items-center rounded px-[7px] text-[11px] font-medium";
</script>

<template>
  <section class="overflow-hidden rounded-[10px] border bg-card" data-testid="node-off-default">
    <h3 class="flex h-[41px] items-center border-b px-4 text-sm font-semibold">
      Sites not on the default
    </h3>
    <ul v-if="sites.length" class="divide-y">
      <li
        v-for="s in sites"
        :key="s.site"
        class="flex h-[38px] items-center gap-3 px-4"
        data-testid="node-off-default-row"
      >
        <span class="min-w-0 flex-1 truncate font-mono text-[13px] font-medium">{{ s.domain }}</span>
        <span v-if="s.source" class="shrink-0 text-[11px] text-faint">{{ s.source }}</span>
        <span
          :class="
            cn(
              CHIP,
              s.unresolved
                ? 'bg-destructive/15 text-destructive'
                : 'bg-secondary text-muted-foreground',
            )
          "
          :title="s.title"
        >
          {{ s.chip }}
        </span>
      </li>
    </ul>
    <div v-else-if="loading" class="flex justify-center py-4"><Spinner class="size-4" /></div>
    <p v-else-if="error" class="px-4 py-3 text-xs text-faint" data-testid="node-off-default-error">
      Couldn't load site versions.
    </p>
    <p v-else class="px-4 py-3 text-xs text-faint" data-testid="node-off-default-empty">
      Every site uses the global default.
    </p>
  </section>
</template>
