<script setup lang="ts">
import { ChevronDown, ChevronRight, CircleCheck, Wrench } from "lucide-vue-next";
import { computed, ref } from "vue";

import Button from "@/components/ui/Button.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useAutoScroll } from "@/composables/useAutoScroll";
import type { ActiveStarter } from "@/composables/useSiteStarters";
import type { ToolPrereqs } from "@/composables/useToolPrereqs";

/**
 * Shown after Project when the picked starter's toolchain is incomplete: one row
 * per tool with its state, and the streamed install log on demand.
 */
const props = defineProps<{
  model: ActiveStarter;
  prereqs: ToolPrereqs;
}>();

const COUNT_WORDS = ["no", "one", "two", "three", "four"];

const missing = computed(() => props.model.prereqRows.filter((r) => !r.ok).length);

const title = computed(() => {
  const n = missing.value;
  const count = COUNT_WORDS[n] ?? String(n);
  return `${props.model.label} needs ${count} more ${n === 1 ? "tool" : "tools"}`;
});

const showLog = ref(false);
const logBox = useAutoScroll(() => props.prereqs.installLog.length);
</script>

<template>
  <div data-testid="ns-missing-tools">
    <div class="flex items-start gap-3.5">
      <span
        class="flex size-9 shrink-0 items-center justify-center rounded-full border border-warning/40 bg-warning/10 text-warning"
      >
        <Wrench class="size-4" aria-hidden="true" />
      </span>
      <div class="min-w-0 pt-0.5">
        <p class="text-[15px] font-semibold">{{ title }}</p>
        <p class="mt-1 text-[13px] text-muted-foreground">
          Yerd installs them for you, once. Nothing is added to your shell profile.
        </p>
        <p class="mt-1 text-xs text-faint">{{ model.prereqSummary }}</p>
      </div>
    </div>

    <div class="mt-4 divide-y rounded-[10px] border">
      <div
        v-for="row in model.prereqRows"
        :key="row.id"
        class="flex min-h-[52px] items-center justify-between gap-3 px-3.5 py-2"
        :data-tool="row.id"
      >
        <div class="min-w-0">
          <p class="text-[13px] font-semibold">{{ row.label }}</p>
          <p class="text-[11px] text-faint">{{ row.sub }}</p>
        </div>
        <span v-if="row.ok" class="flex shrink-0 items-center gap-1.5 text-xs font-medium text-success">
          <CircleCheck class="size-3.5" /> Installed
        </span>
        <span
          v-else-if="prereqs.installingTool === row.id"
          class="flex shrink-0 items-center gap-1.5 text-xs text-brand"
        >
          <Spinner class="size-3.5" /> Installing
        </span>
        <span v-else-if="prereqs.installingAll" class="shrink-0 text-xs text-faint">Queued</span>
        <Button
          v-else
          size="sm"
          variant="outline"
          :disabled="prereqs.busy || !!row.blockedReason"
          :title="row.blockedReason ?? ''"
          @click="row.install()"
        >
          Install
        </Button>
      </div>
    </div>

    <template v-if="prereqs.installLog.length">
      <button
        type="button"
        class="mt-3 flex items-center gap-1 text-xs font-medium text-muted-foreground hover:text-foreground"
        @click="showLog = !showLog"
      >
        <component :is="showLog ? ChevronDown : ChevronRight" class="size-3.5" />
        {{ showLog ? "Hide install log" : "Show install log" }}
      </button>
      <pre
        v-if="showLog"
        ref="logBox"
        class="mt-2 h-36 overflow-y-auto whitespace-pre-wrap rounded-[10px] bg-zinc-950 p-3 font-mono text-[11px] leading-relaxed text-zinc-200"
      >{{ prereqs.installLog.join("\n") }}</pre>
    </template>
  </div>
</template>
