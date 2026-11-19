<script setup lang="ts">
import { ArrowRight, ShieldAlert, TriangleAlert } from "lucide-vue-next";
import { computed } from "vue";
import { useRouter } from "vue-router";

import type { SiteWarning } from "@/lib/sites";

/**
 * An amber strip under the Sites toolbar. The text before an em dash is the
 * headline and is set bold, as drawn; the action opens Doctor.
 */
const props = defineProps<{
  warning: SiteWarning;
  /** Use the shorter copy (narrow windows). */
  compact: boolean;
}>();

const router = useRouter();

const parts = computed(() => {
  const text = props.compact ? props.warning.compactText : props.warning.text;
  const i = text.indexOf(" — ");
  return i === -1 ? { head: text, tail: "" } : { head: text.slice(0, i), tail: text.slice(i) };
});
</script>

<template>
  <div
    class="flex min-h-10 shrink-0 items-center gap-3 border-b border-warning/30 bg-warning/10 px-6 py-2"
    role="status"
    :data-warning="warning.key"
  >
    <ShieldAlert v-if="warning.key === 'ca'" class="size-4 shrink-0 text-warning" aria-hidden="true" />
    <TriangleAlert v-else class="size-4 shrink-0 text-warning" aria-hidden="true" />
    <p class="min-w-0 flex-1 text-[13px]">
      <span :class="parts.tail ? 'font-semibold' : ''">{{ parts.head }}</span>{{ parts.tail }}
    </p>
    <button
      type="button"
      class="flex shrink-0 items-center gap-1 text-[13px] font-semibold text-warning hover:underline"
      @click="router.push('/doctor')"
    >
      {{ warning.action }} <ArrowRight class="size-3.5" aria-hidden="true" />
    </button>
  </div>
</template>
