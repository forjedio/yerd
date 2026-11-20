<script setup lang="ts">
import { X } from "lucide-vue-next";
import {
  DialogClose,
  DialogContent,
  DialogOverlay,
  DialogPortal,
  DialogRoot,
  DialogTitle,
} from "reka-ui";

import { cn } from "@/lib/utils";

import { keepOpenForToaster } from "./toaster";

/**
 * The draft's larger dialog chrome (New site, Link an existing folder): a title
 * row with room for a stepper, a scrolling body, and a footer band with a hint
 * on the left and the actions on the right. Built on reka-ui's Dialog for the
 * focus trap, Escape and outside-click dismissal; `aria-modal` marks it as a
 * modal layer so an inspector drawer beneath leaves Escape to it (reka sets
 * only `role="dialog"`). A click on a toast is not an outside click: the
 * toaster stays interactive over the modal and must not dismiss it. Small
 * utility dialogs keep `Modal.vue`, whose look this deliberately does not
 * change.
 */
withDefaults(
  defineProps<{
    open: boolean;
    title: string;
    /** 440px (`md`) or 572px (`lg`), as drawn; `xl` (760px) for log output. */
    width?: "md" | "lg" | "xl";
  }>(),
  { width: "lg" },
);

const emit = defineEmits<{ "update:open": [boolean] }>();
</script>

<template>
  <DialogRoot :open="open" @update:open="emit('update:open', $event)">
    <DialogPortal>
      <DialogOverlay class="fixed inset-0 z-50 rounded-[10px] bg-black/50 animate-fade-in" />
      <DialogContent
        :aria-describedby="undefined"
        aria-modal="true"
        @interact-outside="keepOpenForToaster"
        :class="
          cn(
            'fixed left-1/2 top-1/2 z-50 flex max-h-[calc(100%-2rem)] w-[calc(100%-2rem)] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-xl border bg-card text-card-foreground shadow-lg animate-fade-in focus:outline-none',
            width === 'md' && 'max-w-[440px]',
            width === 'lg' && 'max-w-[572px]',
            width === 'xl' && 'max-w-[760px]',
          )
        "
      >
        <div class="flex shrink-0 items-center gap-3 px-6 pb-2 pt-5">
          <DialogTitle class="flex min-w-0 flex-1 items-center gap-2 text-[17px] font-semibold">
            <slot name="title">{{ title }}</slot>
          </DialogTitle>
          <slot name="header-extra" />
          <DialogClose
            aria-label="Close"
            class="-mr-1 rounded-md p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          >
            <X class="size-4" />
          </DialogClose>
        </div>
        <div class="min-h-0 flex-1 overflow-y-auto px-6 pb-5 pt-3">
          <slot />
        </div>
        <div class="flex shrink-0 items-center gap-2 border-t bg-background px-6 py-4">
          <div class="mr-auto flex min-w-0 items-center gap-2 text-xs text-faint">
            <slot name="footer-start" />
          </div>
          <slot name="footer" />
        </div>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>
