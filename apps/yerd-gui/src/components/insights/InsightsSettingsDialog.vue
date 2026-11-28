<script setup lang="ts">
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import Input from "@/components/ui/Input.vue";
import Spinner from "@/components/ui/Spinner.vue";
import Switch from "@/components/ui/Switch.vue";
import { SECTION_LABEL } from "@/components/ui/styles";
import { dumpsStatus, setDumpFeature, setDumpsEnabled } from "@/ipc/client";
import type { DumpExtStatus, DumpsStatusResponse, InsightsThresholds } from "@/ipc/types";
import { DUMP_SIGNALS } from "@/lib/dumps";
import { parseThresholds, THRESHOLD_FIELDS, type ThresholdDraft, thresholdDraft } from "@/lib/insights";
import { toastOnError } from "@/lib/toastActions";
import { CHIP_TONE, cn } from "@/lib/utils";

/**
 * Insights settings, opened from the Insights toolbar: the five issue-detection
 * thresholds (saved together, applied to new requests only), the global
 * extension-capture switch, the dump server address with a link to Settings ▸
 * Application Ports where the port lives, the captured-signal switches and the
 * per-version PHP extension status. The capture controls load their own status
 * each time the dialog opens and apply immediately; a failed load says so with
 * a Retry instead of showing a stopped server. Thresholds that arrive after the
 * dialog opened fill the form unless the user has started editing.
 */
const props = defineProps<{
  open: boolean;
  thresholds: InsightsThresholds | null;
  saving: boolean;
}>();

const emit = defineEmits<{
  "update:open": [boolean];
  save: [InsightsThresholds];
  changePort: [];
}>();

const EMPTY: ThresholdDraft = {
  n_plus_one_min: "",
  duplicate_min: "",
  slow_query_ms: "",
  large_read_rows: "",
  max_queries: "",
};

const draft = ref<ThresholdDraft>({ ...EMPTY });
const touched = ref(false);
const seeded = ref(false);
const capture = ref<DumpsStatusResponse | null>(null);
const captureError = ref<string | null>(null);
const captureBusy = ref<string | null>(null);

const parsed = computed(() => parseThresholds(draft.value));
const errors = computed(() => (parsed.value.ok ? {} : parsed.value.errors));
const enabled = computed(() => capture.value?.enabled ?? false);
const extensions = computed(() => capture.value?.extensions ?? []);

watch(
  () => props.open,
  (open) => {
    if (!open) return;
    draft.value = props.thresholds ? thresholdDraft(props.thresholds) : { ...EMPTY };
    seeded.value = !!props.thresholds;
    touched.value = false;
    void loadCapture();
  },
  { immediate: true },
);

watch(
  () => props.thresholds,
  (t) => {
    if (!props.open || seeded.value || touched.value || !t) return;
    draft.value = thresholdDraft(t);
    seeded.value = true;
  },
);

async function loadCapture(): Promise<void> {
  try {
    capture.value = await dumpsStatus();
    captureError.value = null;
  } catch (e) {
    capture.value = null;
    captureError.value = e instanceof Error ? e.message : String(e);
  }
}

function edit(key: keyof ThresholdDraft, value: string): void {
  draft.value = { ...draft.value, [key]: value };
  touched.value = true;
}

function save(): void {
  touched.value = true;
  if (parsed.value.ok) emit("save", parsed.value.thresholds);
}

function featureOn(key: string): boolean {
  return capture.value?.features?.[key] ?? true;
}

async function toggleCapture(next: boolean): Promise<void> {
  captureBusy.value = "enabled";
  await toastOnError(next ? "Couldn't turn capture on" : "Couldn't turn capture off", () => setDumpsEnabled(next));
  await loadCapture();
  captureBusy.value = null;
}

async function toggleFeature(key: string, next: boolean): Promise<void> {
  captureBusy.value = `feature:${key}`;
  await toastOnError("Couldn't change the captured signals", () => setDumpFeature(key, next));
  await loadCapture();
  captureBusy.value = null;
}

function extChip(ext: DumpExtStatus): { text: string; class: string } {
  if (ext.legacy) return { text: "TOO OLD: TIMING ONLY", class: CHIP_TONE.warning };
  if (ext.present) return { text: "INSTALLED", class: CHIP_TONE.success };
  return { text: "NOT INSTALLED", class: CHIP_TONE.warning };
}
</script>

<template>
  <DialogFrame :open="open" title="Insights settings" width="lg" @update:open="emit('update:open', $event)">
    <p :class="SECTION_LABEL">Issue thresholds</p>
    <p class="mt-1 text-xs text-muted-foreground">Changes apply to requests recorded from now on.</p>
    <div class="mt-3 flex flex-col gap-3">
      <div v-for="f in THRESHOLD_FIELDS" :key="f.key" class="flex items-start gap-4" :data-threshold="f.key">
        <div class="min-w-0 flex-1">
          <p class="text-[13px] font-semibold">{{ f.label }}</p>
          <p class="mt-0.5 text-xs text-muted-foreground">{{ f.hint }} {{ draft[f.key] || "N" }} {{ f.unit }}</p>
          <p v-if="touched && errors[f.key]" class="mt-0.5 text-xs text-destructive" data-testid="threshold-error">
            {{ errors[f.key] }}
          </p>
        </div>
        <Input
          :model-value="draft[f.key]"
          inputmode="numeric"
          :aria-label="f.label"
          :aria-invalid="touched && !!errors[f.key]"
          :class="cn('h-8 w-24 shrink-0 text-right font-mono', touched && errors[f.key] && 'border-destructive')"
          @update:model-value="edit(f.key, $event)"
        />
      </div>
    </div>

    <div class="mt-5 border-t pt-4">
      <div class="flex items-start gap-4">
        <div class="min-w-0 flex-1">
          <p class="text-sm font-semibold">Capture from the PHP extension</p>
          <p class="mt-0.5 text-xs text-muted-foreground">
            Queries, dumps, logs and jobs for recording sites. Off keeps timing only.
          </p>
        </div>
        <Switch
          class="mt-0.5"
          :model-value="enabled"
          :disabled="!capture || captureBusy === 'enabled'"
          aria-label="Capture from the PHP extension"
          @update:model-value="toggleCapture"
        />
      </div>

      <div class="mt-4 flex items-end gap-4">
        <div class="min-w-0 flex-1">
          <p class="text-sm font-semibold">Dump server</p>
          <p v-if="capture" class="mt-0.5 text-xs text-muted-foreground" data-testid="dump-server-address">
            <span class="font-mono text-foreground">127.0.0.1:{{ capture.port }}</span>
            · {{ capture.running ? "running" : "stopped" }}
          </p>
          <p v-else-if="!captureError" class="mt-0.5 text-xs text-muted-foreground">Checking…</p>
        </div>
        <button
          type="button"
          class="shrink-0 text-xs font-semibold text-brand hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          @click="emit('changePort')"
        >
          Change port
        </button>
      </div>

      <div
        v-if="captureError"
        class="mt-3 flex items-start gap-3 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2"
        data-testid="capture-error"
      >
        <p class="min-w-0 flex-1 text-xs text-destructive">Couldn't read the capture status: {{ captureError }}</p>
        <button
          type="button"
          class="shrink-0 text-xs font-semibold text-brand hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          @click="loadCapture"
        >
          Retry
        </button>
      </div>
    </div>

    <div class="mt-5 border-t pt-4">
      <p :class="SECTION_LABEL">Captured signals</p>
      <div :class="cn('mt-3 grid gap-x-6 gap-y-3 sm:grid-cols-2 transition-opacity', !enabled && 'opacity-60')">
        <div v-for="signal in DUMP_SIGNALS" :key="signal.key" class="flex items-center justify-between gap-4">
          <span class="text-[13px]">{{ signal.label }}</span>
          <Switch
            :model-value="featureOn(signal.key)"
            :disabled="!enabled || captureBusy === `feature:${signal.key}`"
            :aria-label="signal.label"
            @update:model-value="toggleFeature(signal.key, $event)"
          />
        </div>
      </div>
      <p v-if="capture && !enabled" class="mt-3 text-xs text-muted-foreground">
        Turn on Capture from the PHP extension to change these.
      </p>
    </div>

    <div class="mt-5 border-t pt-4">
      <p :class="SECTION_LABEL">PHP extension</p>
      <p v-if="!capture" class="mt-3 text-[13px] text-muted-foreground" data-testid="ext-unknown">
        {{ captureError ? "Unavailable until the capture status loads." : "Checking…" }}
      </p>
      <p v-else-if="extensions.length === 0" class="mt-3 text-[13px] text-muted-foreground">No PHP versions installed.</p>
      <ul v-else class="mt-2 flex flex-col">
        <li
          v-for="ext in extensions"
          :key="ext.version"
          class="flex h-[30px] items-center justify-between gap-4"
          :data-version="ext.version"
        >
          <span class="font-mono text-[13px]">PHP {{ ext.version }}</span>
          <span
            :class="cn('inline-flex h-5 items-center rounded px-1.5 text-[11px] font-semibold', extChip(ext).class)"
            data-testid="ext-chip"
          >
            {{ extChip(ext).text }}
          </span>
        </li>
      </ul>
    </div>

    <template #footer>
      <Button variant="outline" @click="emit('update:open', false)">Cancel</Button>
      <Button :disabled="saving || (touched && !parsed.ok)" data-testid="save-thresholds" @click="save">
        <Spinner v-if="saving" class="text-current" />
        Save
      </Button>
    </template>
  </DialogFrame>
</template>
