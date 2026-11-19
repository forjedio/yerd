<script setup lang="ts">
import { computed } from "vue";

/**
 * "How a site picks its Node": the daemon's resolution order, first match
 * wins. The last step names the installed global default.
 */
const props = defineProps<{ defaultVersion?: string }>();

const steps = computed(() => [
  { title: ".nvmrc", mono: true, note: "in the project folder" },
  { title: ".node-version", mono: true, note: "in the project folder" },
  { title: "Saved preference", mono: false, note: "set per site in Sites → Node version" },
  {
    title: "Global default",
    mono: false,
    note: props.defaultVersion ? `Node ${props.defaultVersion}` : "None set",
  },
]);
</script>

<template>
  <section class="rounded-[10px] border bg-card p-4" data-testid="node-pick-order">
    <h3 class="text-sm font-semibold">How a site picks its Node</h3>
    <p class="mt-1 text-xs leading-relaxed text-faint">
      First match wins. Builds in the New site wizard save their choice as the preference.
    </p>
    <ol class="mt-3.5 space-y-3">
      <li v-for="(s, i) in steps" :key="s.title" class="flex items-start gap-2.5">
        <span
          class="flex size-[22px] shrink-0 items-center justify-center rounded-full bg-secondary text-[11px] font-medium text-muted-foreground"
          aria-hidden="true"
        >
          {{ i + 1 }}
        </span>
        <span class="min-w-0">
          <span :class="['block text-[13px] font-semibold leading-tight', s.mono && 'font-mono']">
            {{ s.title }}
          </span>
          <span class="mt-0.5 block text-xs text-faint">{{ s.note }}</span>
        </span>
      </li>
    </ol>
  </section>
</template>
