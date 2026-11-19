<script setup lang="ts">
import { computed } from "vue";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useToast } from "@/composables/useToast";
import { openInEditor } from "@/ipc/client";
import type { DumpEvent } from "@/ipc/types";
import {
  cacheTag,
  CATEGORY_LABEL,
  methodTag,
  rowBody,
  rowCaller,
  rowCallerRel,
  rowCount,
  rowDuration,
  statusTag,
  str,
  type Tag,
  TAG,
  TAG_TONE,
} from "@/lib/dumps";
import { cn } from "@/lib/utils";

/**
 * One captured event: its category and measure tags, the caller badge and the
 * body. A caller with an absolute path opens the file in the default editor;
 * anything else (e.g. "Command line code") is shown but not clickable. The
 * parent supplies the `TooltipProvider`.
 */
const props = defineProps<{ event: DumpEvent }>();

const toast = useToast();

const tags = computed<Tag[]>(() => {
  const e = props.event;
  const out: Tag[] = [{ text: CATEGORY_LABEL[e.category], tone: "neutral" }];
  for (const tag of [cacheTag(e), methodTag(e), statusTag(e)]) if (tag) out.push(tag);
  const duration = rowDuration(e);
  if (duration) out.push({ text: duration, tone: "neutral" });
  const count = rowCount(e);
  if (count) out.push({ text: count, tone: "neutral" });
  return out;
});

const caller = computed(() => rowCaller(props.event));
const callerRel = computed(() => rowCallerRel(props.event));
const file = computed(() => str(props.event.payload.file));
const openable = computed(() => file.value.startsWith("/"));
const body = computed(() => rowBody(props.event));

async function openCaller(): Promise<void> {
  try {
    await openInEditor(file.value);
  } catch (e) {
    toast.error("Couldn't open file", `${callerRel.value}: ${e instanceof Error ? e.message : String(e)}`);
  }
}

const BADGE = "ml-auto block h-[18px] min-w-0 truncate rounded bg-muted px-1.5 font-mono text-[11px] leading-[18px] text-muted-foreground";
</script>

<template>
  <div class="rounded-[10px] border bg-card p-3.5" :data-id="event.id" data-testid="dump-event">
    <div class="flex items-center gap-1.5">
      <span v-for="(tag, i) in tags" :key="i" :class="cn(TAG, TAG_TONE[tag.tone])" data-testid="dump-tag">
        {{ tag.text }}
      </span>
      <Tooltip v-if="caller">
        <TooltipTrigger as-child>
          <button
            v-if="openable"
            type="button"
            :class="cn(BADGE, 'transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring')"
            :aria-label="`Open ${callerRel}`"
            data-testid="dump-caller"
            @click="openCaller"
          >
            {{ caller }}
          </button>
          <span v-else :class="BADGE" data-testid="dump-caller">{{ caller }}</span>
        </TooltipTrigger>
        <TooltipContent side="top" class="max-w-sm font-mono text-[11px]">
          {{ openable ? `${callerRel} — click to open` : callerRel }}
        </TooltipContent>
      </Tooltip>
    </div>
    <pre
      v-if="body"
      class="mt-2.5 whitespace-pre-wrap break-words font-mono text-[12.5px] leading-relaxed text-foreground"
      data-testid="dump-body"
    >{{ body }}</pre>
  </div>
</template>
