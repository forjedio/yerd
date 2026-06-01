<script setup lang="ts">
import { watch } from "vue";

import { cn } from "@/lib/utils";

const props = withDefaults(
  defineProps<{
    open: boolean;
    title: string;
    /** Dialog footprint: "md" (default), "lg", or "full" (~80% of the window). */
    size?: "md" | "lg" | "full";
  }>(),
  { size: "md" },
);
const emit = defineEmits<{ "update:open": [boolean] }>();

function close(): void {
  emit("update:open", false);
}

function onKey(e: KeyboardEvent): void {
  if (e.key === "Escape") close();
}

// Toggle a global key listener only while open.
watch(
  () => props.open,
  (isOpen) => {
    if (isOpen) document.addEventListener("keydown", onKey);
    else document.removeEventListener("keydown", onKey);
  },
);
</script>

<template>
  <Teleport to="body">
    <div
      v-if="open"
      class="fixed inset-0 z-50 flex items-center justify-center p-4"
    >
      <div
        class="absolute inset-0 bg-black/50 animate-fade-in"
        @click="close"
      />
      <div
        role="dialog"
        aria-modal="true"
        :class="
          cn(
            'relative z-10 flex max-h-[90vh] w-full flex-col rounded-lg border bg-background p-6 shadow-lg animate-fade-in',
            size === 'full' && 'h-[80vh] w-[80vw] max-w-none',
            size === 'lg' && 'max-w-2xl',
            size === 'md' && 'max-w-md',
          )
        "
      >
        <h2 class="shrink-0 text-lg font-semibold">{{ title }}</h2>
        <div class="mt-4 min-h-0 flex-1 overflow-auto">
          <slot />
        </div>
        <div class="mt-6 flex shrink-0 justify-end gap-2">
          <slot name="footer" :close="close" />
        </div>
      </div>
    </div>
  </Teleport>
</template>
