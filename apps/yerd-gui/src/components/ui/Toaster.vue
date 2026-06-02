<script setup lang="ts">
import { CheckCircle2, Info, X, XCircle } from "lucide-vue-next";

import { useToast } from "@/composables/useToast";
import { cn } from "@/lib/utils";

const { toasts, dismiss } = useToast();

const icon = { success: CheckCircle2, error: XCircle, info: Info } as const;
const accent = {
  success: "border-l-success",
  error: "border-l-destructive",
  info: "border-l-primary",
} as const;
</script>

<template>
  <Teleport to="body">
    <div class="fixed bottom-4 right-4 z-[60] flex w-80 flex-col gap-2">
      <div
        v-for="t in toasts"
        :key="t.id"
        :class="
          cn(
            'flex items-start gap-3 rounded-md border border-l-4 bg-popover p-3 text-popover-foreground shadow-lg animate-fade-in',
            accent[t.kind],
          )
        "
      >
        <component
          :is="icon[t.kind]"
          class="mt-0.5 size-4 shrink-0"
          :class="{
            'text-success': t.kind === 'success',
            'text-destructive': t.kind === 'error',
            'text-primary': t.kind === 'info',
          }"
        />
        <div class="min-w-0 flex-1">
          <p class="text-sm font-medium">{{ t.title }}</p>
          <p
            v-if="t.detail"
            class="mt-0.5 break-words text-xs text-muted-foreground"
          >
            {{ t.detail }}
          </p>
        </div>
        <button
          class="text-muted-foreground hover:text-foreground"
          aria-label="Dismiss"
          @click="dismiss(t.id)"
        >
          <X class="size-4" />
        </button>
      </div>
    </div>
  </Teleport>
</template>
