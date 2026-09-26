<script setup lang="ts">
import { computed, ref, watch } from "vue";
import {
  Check,
  CheckCircle2,
  ChevronLeft,
  Circle,
  ExternalLink,
  FolderOpen,
  Loader2,
  TriangleAlert,
} from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { phaseStatus, useCreateSiteJob } from "@/composables/useCreateSiteJob";
import type { PrereqRow, ToolPrereqs } from "@/composables/useToolPrereqs";
import { openInBrowser, openPath } from "@/ipc/client";
import type { CreateSiteSpec } from "@/ipc/types";

/**
 * The frame every create-site wizard shares: the toolchain check and
 * prerequisites gate, the step indicator, the create job's progress view and
 * all footers. A wizard supplies its input steps as `step-0`…`step-N` slots
 * (Review included), plus its form's validity per step and its spec.
 */
const props = defineProps<{
  open: boolean;
  title: string;
  /** Labels of the input steps; the progress view follows the last one. */
  steps: string[];
  /** Whether each input step may be left forwards (Next, or Create site on the last). */
  stepValid: boolean[];
  buildSpec: () => CreateSiteSpec;
  /** The daemon's phase names for this framework's job, in order. */
  phases: string[];
  domain: string;
  projectPath: string;
  openUrl: string;
  prereqs: ToolPrereqs;
  prereqRows: PrereqRow[];
  /** What the prerequisites gate says the framework needs. */
  prereqSummary: string;
  /** Whether the required toolchain is present, unlocking the steps. */
  ready: boolean;
  installAll: () => Promise<void>;
}>();
const emit = defineEmits<{
  (e: "update:open", v: boolean): void;
  (e: "created"): void;
  /** The wizard should restore its form defaults and refresh what it lists. */
  (e: "reset"): void;
}>();

const {
  state: jobState,
  phase,
  log,
  error: jobError,
  cancelRequested,
  inFlight,
  start,
  cancel,
  reset: resetJob,
} = useCreateSiteJob({
  isOpen: () => props.open,
  domain: () => props.domain,
  onCreated: () => emit("created"),
});

const step = ref(0);
const progressStep = computed(() => props.steps.length);
const lastStep = computed(() => props.steps.length - 1);
const busy = computed(() => jobState.value === "running" && step.value === progressStep.value);

const installLogBox = ref<HTMLElement | null>(null);
const logBox = ref<HTMLElement | null>(null);

function scrollToEnd(el: HTMLElement | null): void {
  if (el) el.scrollTop = el.scrollHeight;
}

watch(
  () => props.prereqs.installLog.length,
  () => scrollToEnd(installLogBox.value),
  { flush: "post" },
);
watch(
  () => log.value.length,
  () => scrollToEnd(logBox.value),
  { flush: "post" },
);

function startCreate(): void {
  step.value = progressStep.value;
  void start(props.buildSpec());
}

/** A create keeps running while the dialog is closed; reopening mid-create resumes its progress. */
watch(
  () => props.open,
  (open) => {
    if (open && !inFlight.value) {
      step.value = 0;
      resetJob();
      emit("reset");
    }
  },
  { immediate: true },
);
</script>

<template>
  <Modal :open="open" :title="title" size="lg" @update:open="(v) => emit('update:open', v)">
    <div v-if="prereqs.loading" class="flex items-center justify-center py-12">
      <Spinner class="size-6" />
    </div>

    <div v-else-if="!ready" class="space-y-4">
      <div class="flex items-start gap-2 rounded-lg border border-warning/40 bg-warning/10 p-3">
        <TriangleAlert class="mt-0.5 size-4 shrink-0 text-warning" />
        <div>
          <p class="text-sm font-medium">A few tools are needed first</p>
          <p class="text-xs text-muted-foreground">
            {{ prereqSummary }} Install the missing ones to continue.
          </p>
        </div>
      </div>

      <div class="divide-y rounded-lg border">
        <div
          v-for="row in prereqRows"
          :key="row.id"
          class="flex items-center justify-between gap-3 px-3 py-2.5"
        >
          <div class="min-w-0">
            <p class="text-sm font-medium">{{ row.label }}</p>
            <p class="text-xs text-muted-foreground">{{ row.sub }}</p>
          </div>
          <div class="flex shrink-0 items-center gap-2">
            <span v-if="row.ok" class="flex items-center gap-1 text-xs text-success">
              <CheckCircle2 class="size-4" /> Installed
            </span>
            <template v-else>
              <Spinner v-if="prereqs.installingTool === row.id" class="size-4" />
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
            </template>
          </div>
        </div>
      </div>

      <pre
        v-if="prereqs.installLog.length"
        ref="installLogBox"
        class="h-40 overflow-y-auto whitespace-pre-wrap rounded-lg bg-zinc-950 p-3 font-mono text-[11px] leading-relaxed text-zinc-200"
      >{{ prereqs.installLog.join("\n") }}</pre>
    </div>

    <template v-else>
      <div v-if="step < progressStep" class="mb-6 flex w-full">
        <div
          v-for="(label, i) in steps"
          :key="label"
          class="step-chevron -ml-2.5 flex h-9 flex-1 items-center justify-center gap-1.5 pl-5 pr-1 text-xs font-medium transition-colors first:ml-0 first:pl-3"
          :class="[
            i < step
              ? 'bg-brand/70 text-white'
              : i === step
                ? 'bg-brand text-white'
                : 'bg-muted text-muted-foreground',
            i !== step && i !== 0 ? 'step-chevron-sep' : '',
          ]"
        >
          <Check v-if="i < step" class="size-3.5 shrink-0" />
          <span>{{ label }}</span>
        </div>
      </div>

      <slot v-if="step < progressStep" :name="`step-${step}`" />

      <div v-else class="space-y-4">
        <div
          class="grid"
          :style="{ gridTemplateColumns: `repeat(${phases.length}, minmax(0, 1fr))` }"
        >
          <div v-for="(p, i) in phases" :key="p" class="flex flex-col items-center gap-1.5 px-0.5">
            <div class="relative flex w-full items-center justify-center">
              <span
                v-if="i < phases.length - 1"
                class="absolute left-1/2 top-1/2 z-0 h-0.5 w-full -translate-y-1/2 rounded-full transition-colors"
                :class="
                  phaseStatus(phases, phase, jobState, p) === 'done' ? 'bg-success' : 'bg-border'
                "
              />
              <span
                class="relative z-10 flex size-6 shrink-0 items-center justify-center rounded-full transition-colors"
                :class="
                  phaseStatus(phases, phase, jobState, p) === 'done'
                    ? 'bg-success text-white'
                    : phaseStatus(phases, phase, jobState, p) === 'active'
                      ? 'bg-brand text-white'
                      : 'bg-muted text-muted-foreground'
                "
              >
                <Check v-if="phaseStatus(phases, phase, jobState, p) === 'done'" class="size-3.5" />
                <Loader2
                  v-else-if="phaseStatus(phases, phase, jobState, p) === 'active'"
                  class="size-3.5 animate-spin"
                />
                <Circle v-else class="size-2 fill-current" />
              </span>
            </div>
            <span
              class="text-center text-[11px] leading-tight"
              :class="
                phaseStatus(phases, phase, jobState, p) === 'todo'
                  ? 'text-muted-foreground'
                  : 'font-medium'
              "
            >{{ p }}</span>
          </div>
        </div>
        <p
          v-if="phase && !phases.includes(phase) && jobState === 'running'"
          class="text-xs text-muted-foreground"
        >
          {{ phase }}…
        </p>

        <pre
          ref="logBox"
          class="h-56 overflow-y-auto whitespace-pre-wrap rounded-lg bg-zinc-950 p-3 font-mono text-[11px] leading-relaxed text-zinc-200"
        >{{ log.join("\n") || "Starting…" }}</pre>

        <div
          v-if="jobState === 'succeeded'"
          class="flex items-center gap-2 rounded-lg border border-success/40 bg-success/10 p-3 text-sm text-success"
        >
          <CheckCircle2 class="size-4" /> {{ domain }} is ready.
        </div>
        <div
          v-else-if="jobState === 'failed'"
          class="rounded-lg border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive"
        >
          {{ jobError || "Creation failed." }}
        </div>
        <div
          v-else-if="jobState === 'cancelled'"
          class="rounded-lg border p-3 text-sm text-muted-foreground"
        >
          Cancelled.
        </div>
      </div>
    </template>

    <template #footer="{ close: modalClose }">
      <template v-if="prereqs.loading">
        <Button variant="ghost" @click="modalClose">Cancel</Button>
      </template>

      <template v-else-if="!ready">
        <Button variant="ghost" @click="modalClose">Cancel</Button>
        <Button :disabled="prereqs.busy" @click="installAll">
          <Spinner v-if="prereqs.installingAll" class="size-4" /> Install missing tools
        </Button>
      </template>

      <template v-else-if="step === progressStep">
        <template v-if="busy">
          <Button variant="ghost" :disabled="cancelRequested" @click="cancel">
            {{ cancelRequested ? "Cancelling…" : "Cancel" }}
          </Button>
        </template>
        <template v-else-if="jobState === 'succeeded'">
          <Button variant="outline" @click="openPath(projectPath)">
            <FolderOpen class="size-4" /> Open folder
          </Button>
          <slot name="success-actions" />
          <Button variant="outline" @click="openInBrowser(openUrl)">
            <ExternalLink class="size-4" /> Open in browser
          </Button>
          <Button @click="modalClose">Done</Button>
        </template>
        <template v-else>
          <Button variant="ghost" @click="step = lastStep">
            <ChevronLeft class="size-4" /> Back
          </Button>
          <Button @click="modalClose">Close</Button>
        </template>
      </template>

      <template v-else>
        <Button v-if="step > 0" variant="ghost" @click="step -= 1">
          <ChevronLeft class="size-4" /> Back
        </Button>
        <Button v-else variant="ghost" @click="modalClose">Cancel</Button>

        <Button v-if="step < lastStep" :disabled="!stepValid[step]" @click="step += 1">
          Next
        </Button>
        <Button v-else :disabled="!ready || !stepValid[lastStep]" @click="startCreate">
          Create site
        </Button>
      </template>
    </template>
  </Modal>
</template>

<style scoped>
/* Chevron breadcrumb: each segment points right with a matching notch on its
   left; the first is flat on the left and the last flat on the right. */
.step-chevron {
  clip-path: polygon(
    0 0,
    calc(100% - 10px) 0,
    100% 50%,
    calc(100% - 10px) 100%,
    0 100%,
    10px 50%
  );
}
.step-chevron:first-child {
  clip-path: polygon(0 0, calc(100% - 10px) 0, 100% 50%, calc(100% - 10px) 100%, 0 100%);
}
.step-chevron:last-child {
  clip-path: polygon(0 0, 100% 0, 100% 100%, 0 100%, 10px 50%);
}
/* clip-path eats borders, so a left-offset drop-shadow traces the notch edge
   over the previous segment's arrow as a 1px separator. */
.step-chevron-sep {
  filter: drop-shadow(-2.5px 0 0 #a1a1aa);
}
</style>
