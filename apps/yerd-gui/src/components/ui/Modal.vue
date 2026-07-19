<script setup lang="ts">
import { X } from "lucide-vue-next";
import { onUnmounted, useId, watch } from "vue";

import { cn } from "@/lib/utils";

const titleId = useId();

const props = withDefaults(
  defineProps<{
    open: boolean;
    title: string;
    /** Dialog footprint: "md" (default), "lg", or "full" (~80% of the window). */
    size?: "md" | "lg" | "full";
    /**
     * When false, the dialog can't be dismissed by the backdrop, Escape, or the
     * built-in close (X) button - the only way out is a control the caller
     * renders in the footer (e.g. a "Close" button enabled once work finishes).
     * Defaults to true (the usual dismiss-anywhere modal).
     */
    dismissible?: boolean;
  }>(),
  { size: "md", dismissible: true },
);
const emit = defineEmits<{ "update:open": [boolean] }>();

function close(): void {
  emit("update:open", false);
}

/** Backdrop/Escape dismissal path; a no-op when the dialog is non-dismissible. */
function requestDismiss(): void {
  if (props.dismissible) close();
}

function onKey(e: KeyboardEvent): void {
  if (e.key === "Escape") requestDismiss();
}

// Toggle a global key listener only while open.
watch(
  () => props.open,
  (isOpen) => {
    if (isOpen) document.addEventListener("keydown", onKey);
    else document.removeEventListener("keydown", onKey);
  },
);

// Being torn down while open would otherwise strand the listener for good.
onUnmounted(() => document.removeEventListener("keydown", onKey));
</script>

<template>
  <Teleport to="body">
    <div
      v-if="open"
      class="fixed inset-0 z-50 flex items-center justify-center p-4"
    >
      <div
        class="absolute inset-0 bg-black/50 rounded-[10px] animate-fade-in"
        @click="requestDismiss"
      />
      <div
        role="dialog"
        aria-modal="true"
        :aria-labelledby="titleId"
        :class="
          cn(
            'relative z-10 flex max-h-[90vh] w-full flex-col rounded-lg border bg-background p-6 shadow-lg animate-fade-in',
            size === 'full' && 'h-[80vh] w-[80vw] max-w-none',
            size === 'lg' && 'max-w-2xl',
            size === 'md' && 'max-w-md',
          )
        "
      >
        <div class="flex shrink-0 items-start justify-between gap-4">
          <h2 :id="titleId" class="font-display text-lg font-normal tracking-wide">
            {{ title }}
          </h2>
          <button
            v-if="dismissible"
            type="button"
            aria-label="Close"
            class="-mr-1 -mt-1 rounded-md p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            @click="close"
          >
            <X class="size-5" />
          </button>
        </div>
        <div class="mt-4 min-h-0 flex-1 overflow-auto">
          <slot />
        </div>
        <div class="mt-6 flex shrink-0 items-center justify-end gap-2">
          <slot name="footer" :close="close" />
        </div>
      </div>
    </div>
  </Teleport>
</template>
