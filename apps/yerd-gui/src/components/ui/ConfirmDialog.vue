<script setup lang="ts">
import type { Component } from "vue";
import {
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogOverlay,
  AlertDialogPortal,
  AlertDialogRoot,
  AlertDialogTitle,
} from "reka-ui";

import Button from "@/components/ui/Button.vue";
import { cn } from "@/lib/utils";

/**
 * A confirmation dialog as drawn in the "Yerd App" draft: a tinted icon tile
 * beside the title and body, over a footer band holding Cancel and the action.
 * Built on reka-ui's AlertDialog for the focus trap, Escape and `alertdialog`
 * role. The body text goes in the default slot.
 */
defineProps<{
  open: boolean;
  title: string;
  confirmLabel: string;
  /** `danger` paints the tile and action red; `primary` uses the brand blue. */
  tone: "danger" | "primary";
  icon: Component;
}>();

const emit = defineEmits<{ "update:open": [boolean]; confirm: [] }>();
</script>

<template>
  <AlertDialogRoot :open="open" @update:open="emit('update:open', $event)">
    <AlertDialogPortal>
      <AlertDialogOverlay class="fixed inset-0 z-50 rounded-[10px] bg-black/50 animate-fade-in" />
      <AlertDialogContent
        class="fixed left-1/2 top-1/2 z-50 w-[calc(100%-2rem)] max-w-[440px] -translate-x-1/2 -translate-y-1/2 overflow-hidden rounded-xl border bg-card text-card-foreground shadow-lg animate-fade-in focus:outline-none"
      >
        <div class="flex items-start gap-4 px-5 py-5">
          <span
            :class="
              cn(
                'flex size-10 shrink-0 items-center justify-center rounded-full ring-1',
                tone === 'danger'
                  ? 'bg-destructive/10 text-destructive ring-destructive/30'
                  : 'bg-brand/10 text-brand ring-brand/30',
              )
            "
          >
            <component :is="icon" class="size-4" aria-hidden="true" />
          </span>
          <div class="min-w-0 flex-1 pt-0.5">
            <AlertDialogTitle class="text-[17px] font-semibold leading-snug">
              {{ title }}
            </AlertDialogTitle>
            <AlertDialogDescription class="mt-1.5 text-[13px] leading-relaxed text-muted-foreground">
              <slot />
            </AlertDialogDescription>
          </div>
        </div>
        <div class="flex items-center justify-end gap-2 border-t bg-background px-5 py-4">
          <AlertDialogCancel as-child>
            <Button variant="outline">Cancel</Button>
          </AlertDialogCancel>
          <AlertDialogAction as-child>
            <Button :variant="tone === 'danger' ? 'destructive' : 'default'" @click="emit('confirm')">
              <component :is="icon" aria-hidden="true" />
              {{ confirmLabel }}
            </Button>
          </AlertDialogAction>
        </div>
      </AlertDialogContent>
    </AlertDialogPortal>
  </AlertDialogRoot>
</template>
