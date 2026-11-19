<script setup lang="ts">
import { nextTick, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import Spinner from "@/components/ui/Spinner.vue";

/**
 * A Node install's streamed log. Hiding it never stops the install: the job
 * keeps polling in the view, the sidebar shows the operation and a toast
 * reports the outcome.
 */
const props = defineProps<{
  open: boolean;
  title: string;
  phase: "running" | "done" | "error";
  lines: string[];
}>();

const emit = defineEmits<{ "update:open": [boolean] }>();

const logBox = ref<HTMLElement | null>(null);

watch(
  () => props.lines.length,
  async () => {
    await nextTick();
    const el = logBox.value;
    if (el) el.scrollTop = el.scrollHeight;
  },
);
</script>

<template>
  <DialogFrame
    :open="open"
    :title="title"
    width="lg"
    @update:open="(v: boolean) => emit('update:open', v)"
  >
    <pre
      ref="logBox"
      class="h-72 overflow-y-auto whitespace-pre-wrap rounded-lg border bg-background p-3 font-mono text-[11px] leading-relaxed text-muted-foreground"
      data-testid="node-install-log"
    ><template v-if="lines.length"><span
      v-for="(l, i) in lines"
      :key="i"
      :class="['block', phase === 'running' && i === lines.length - 1 && 'text-brand']"
    >{{ l }}</span></template><template v-else>Starting…</template></pre>
    <p v-if="phase === 'running'" class="mt-2.5 text-xs text-faint">
      You can close this window; the install keeps running in the background.
    </p>

    <template #footer>
      <Spinner v-if="phase === 'running'" class="size-4 text-brand" />
      <Button @click="emit('update:open', false)">
        {{ phase === "running" ? "Hide" : "Done" }}
      </Button>
    </template>
  </DialogFrame>
</template>
