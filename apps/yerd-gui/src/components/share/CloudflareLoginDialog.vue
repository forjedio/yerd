<script setup lang="ts">
import { Globe } from "lucide-vue-next";

import { CARD } from "@/components/ui/styles";
import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useAutoScroll } from "@/composables/useAutoScroll";
import { cn } from "@/lib/utils";

/**
 * The streamed `cloudflared tunnel login`. cloudflared opens the browser itself,
 * so the captured authorization URL is only offered as a manual fallback (opening
 * it automatically produced duplicate browser windows).
 */
const props = defineProps<{
  open: boolean;
  lines: string[];
  url: string | null;
  running: boolean;
}>();

const emit = defineEmits<{ "update:open": [boolean]; openUrl: [url: string] }>();

const logBox = useAutoScroll(() => props.lines.length);
</script>

<template>
  <DialogFrame
    :open="open"
    title="Connect Cloudflare account"
    width="lg"
    @update:open="(v: boolean) => emit('update:open', v)"
  >
    <p class="text-[13px] text-muted-foreground">
      A browser window should have opened to authorise Yerd. If it didn't, open the link below.
    </p>
    <div v-if="url" :class="cn(CARD, 'mt-3 flex h-12 items-center gap-3 pl-3 pr-2')" data-testid="login-url">
      <span class="min-w-0 flex-1 truncate font-mono text-xs">{{ url }}</span>
      <Button size="sm" variant="outline" class="shrink-0" @click="emit('openUrl', url)">
        <Globe /> Open in browser
      </Button>
    </div>
    <pre
      ref="logBox"
      class="mt-3 h-40 overflow-y-auto whitespace-pre-wrap rounded-lg border bg-sidebar p-3 font-mono text-[11px] leading-relaxed text-muted-foreground"
      data-testid="login-log"
    >{{ lines.join("\n") || "Waiting for Cloudflare login…" }}</pre>

    <template #footer>
      <Spinner v-if="running" class="size-4 text-brand" />
      <Button variant="outline" :disabled="running" @click="emit('update:open', false)">Done</Button>
    </template>
  </DialogFrame>
</template>
