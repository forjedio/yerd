<script setup lang="ts">
import { CheckCircle2, TriangleAlert } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useAutoScroll } from "@/composables/useAutoScroll";

/**
 * A tool install's streamed log. The title is fixed when the install starts and
 * its spinner turns into a check or a warning when the job ends. Closing it
 * never stops the install: the view keeps polling, the sidebar shows the
 * operation and a toast reports the outcome.
 */
const props = defineProps<{
  open: boolean;
  title: string;
  phase: "running" | "done" | "error";
  lines: string[];
}>();

const emit = defineEmits<{ "update:open": [boolean] }>();

const logBox = useAutoScroll(() => props.lines.length);
</script>

<template>
  <DialogFrame
    :open="open"
    :title="title"
    width="xl"
    @update:open="(v: boolean) => emit('update:open', v)"
  >
    <template #title>
      <Spinner v-if="phase === 'running'" class="size-4 shrink-0 text-brand" />
      <CheckCircle2 v-else-if="phase === 'done'" class="size-4 shrink-0 text-success" />
      <TriangleAlert v-else class="size-4 shrink-0 text-destructive" />
      <span class="truncate" data-testid="tool-install-title">{{ title }}</span>
    </template>

    <div
      v-if="phase === 'running'"
      class="mb-3 h-1.5 overflow-hidden rounded-full bg-muted"
      role="progressbar"
      :aria-label="title"
    >
      <div class="h-full w-1/3 animate-pulse rounded-full bg-brand/60" />
    </div>
    <pre
      ref="logBox"
      class="h-[min(24rem,50vh)] overflow-y-auto whitespace-pre-wrap rounded-lg border bg-background p-3 font-mono text-[11px] leading-relaxed text-muted-foreground"
      data-testid="tool-install-log"
    ><template v-if="lines.length"><span
      v-for="(l, i) in lines"
      :key="i"
      :class="['block', phase === 'running' && i === lines.length - 1 && 'text-brand']"
    >{{ l }}</span></template><template v-else>Starting…</template></pre>

    <template #footer-start>
      <span v-if="phase === 'running'">Keeps going if you close this</span>
    </template>
    <template #footer>
      <Button variant="outline" @click="emit('update:open', false)">Close</Button>
    </template>
  </DialogFrame>
</template>
