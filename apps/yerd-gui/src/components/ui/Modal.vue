<script setup lang="ts">
import { watch } from "vue";

import { cn } from "@/lib/utils";

const props = defineProps<{ open: boolean; title: string }>();
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
            'relative z-10 w-full max-w-md rounded-lg border bg-background p-6 shadow-lg animate-fade-in',
          )
        "
      >
        <h2 class="text-lg font-semibold">{{ title }}</h2>
        <div class="mt-4">
          <slot />
        </div>
        <div class="mt-6 flex justify-end gap-2">
          <slot name="footer" :close="close" />
        </div>
      </div>
    </div>
  </Teleport>
</template>
