<script setup lang="ts">
import { RefreshCw, Undo2, Wrench } from "lucide-vue-next";
import { computed, onMounted, onUnmounted, ref } from "vue";

import DoctorInspector, { type DoctorInspectorState } from "@/components/doctor/DoctorInspector.vue";
import DoctorList from "@/components/doctor/DoctorList.vue";
import PageHeader from "@/components/PageHeader.vue";
import Button from "@/components/ui/Button.vue";
import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useElevation } from "@/composables/useElevation";
import { useToast } from "@/composables/useToast";
import { diagnose, doctorFix, IpcError } from "@/ipc/client";
import type { Diagnosis } from "@/ipc/types";
import { defaultSelection, findingGroups, hasActionable, permissionKey } from "@/lib/doctor";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { copyWithToast } from "@/lib/toastActions";

const toast = useToast();
const { refresh: refreshStatus } = useDaemon();

const diagnoses = ref<Diagnosis[]>([]);
const loaded = ref(false);
const diagLoading = ref(true);
const diagError = ref(false);
const fixing = ref(false);
const selected = ref<string | null>(null);

const {
  canElevate,
  busy,
  items,
  fixableCount,
  fixAll,
  elevateOne,
  unelevateOpen,
  unelevateCopy,
  openUnelevate,
  confirmUnelevate,
} = useElevation({ onSettled: () => void loadDiagnoses() });

const groups = computed(() => (diagError.value ? [] : findingGroups(diagnoses.value)));
const actionable = computed(() => !diagError.value && hasActionable(diagnoses.value));
const canFixAll = computed(() => canElevate.value && fixableCount.value >= 2);

const selectedFinding = computed(
  () => groups.value.flatMap((g) => g.items).find((i) => i.key === selected.value)?.finding ?? null,
);
const selectedItem = computed(
  () => items.value.find((i) => permissionKey(i.target) === selected.value) ?? null,
);

const inspectorState = computed<DoctorInspectorState>(() => {
  if (selectedItem.value) return "permission";
  if (!loaded.value) return "loading";
  if (diagError.value) return "error";
  if (selectedFinding.value) return "finding";
  return "clear";
});

function syncSelection(): void {
  const keys = new Set([
    ...groups.value.flatMap((g) => g.items.map((i) => i.key)),
    ...items.value.map((i) => permissionKey(i.target)),
  ]);
  if (selected.value === null || !keys.has(selected.value)) {
    selected.value = defaultSelection(groups.value);
  }
}

/** Bumped per diagnose run so a slower, superseded run can't overwrite a newer one. */
let diagRun = 0;

async function loadDiagnoses(notify = false): Promise<void> {
  const run = ++diagRun;
  diagLoading.value = true;
  diagError.value = false;
  try {
    const ds = await diagnose();
    if (run !== diagRun) return;
    diagnoses.value = ds;
    if (notify) toast.success("Health re-checked");
  } catch (e) {
    if (run !== diagRun) return;
    diagError.value = true;
    toast.error("Couldn't run diagnostics", (e as IpcError).message);
  } finally {
    if (run === diagRun) {
      diagLoading.value = false;
      loaded.value = true;
      syncSelection();
    }
  }
}

async function recheck(): Promise<void> {
  await Promise.all([loadDiagnoses(true), refreshStatus()]);
}

async function runFixes(): Promise<void> {
  fixing.value = true;
  try {
    const r = await doctorFix();
    const ok = r.performed.filter((p) => p.ok).length;
    toast.success(
      "Ran safe fixes",
      `${ok}/${r.performed.length} applied · ${r.manual.length} need manual action`,
    );
    await Promise.all([loadDiagnoses(), refreshStatus()]);
  } catch (e) {
    toast.error("Fix run failed", (e as IpcError).message);
  } finally {
    fixing.value = false;
  }
}

async function copyRemedy(text: string): Promise<void> {
  await copyWithToast(text, { title: "Copied to clipboard" });
}

onMounted(() => void loadDiagnoses());
onUnmounted(registerViewActions({ refresh: () => void recheck() }));
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <PageHeader title="Doctor" docs="/guide/diagnostics">
      <template #actions>
        <Button
          variant="outline"
          size="icon"
          class="size-8 shrink-0"
          :disabled="diagLoading"
          aria-label="Re-check health"
          @click="recheck"
        >
          <Spinner v-if="diagLoading" class="size-4" />
          <RefreshCw v-else />
        </Button>
        <Button size="sm" :disabled="!actionable || fixing" @click="runFixes">
          <Spinner v-if="fixing" class="size-4" />
          <Wrench v-else />
          Run safe fixes
        </Button>
      </template>
    </PageHeader>

    <div class="flex min-h-0 flex-1">
      <div class="flex w-[300px] shrink-0 flex-col xl:w-[360px]">
        <DoctorList
          :groups="groups"
          :loading="!loaded"
          :items="items"
          :selected="selected"
          :can-fix-all="canFixAll"
          :busy="busy"
          @select="selected = $event"
          @fix-all="fixAll"
        />
      </div>
      <div class="min-w-0 flex-1 border-l">
        <DoctorInspector
          :state="inspectorState"
          :finding="selectedFinding"
          :item="selectedItem"
          :can-elevate="canElevate"
          :busy="busy"
          :checking="diagLoading"
          @copy="copyRemedy"
          @fix="elevateOne"
          @revert="openUnelevate"
          @recheck="recheck"
        />
      </div>
    </div>

    <ConfirmDialog
      v-model:open="unelevateOpen"
      :title="unelevateCopy?.title ?? 'Revert privilege'"
      confirm-label="Revert"
      tone="primary"
      :icon="Undo2"
      @confirm="confirmUnelevate"
    >
      <span class="block">{{ unelevateCopy?.body }}</span>
      <span v-if="unelevateCopy?.note" class="mt-3 block text-xs text-faint">{{ unelevateCopy.note }}</span>
    </ConfirmDialog>
  </div>
</template>
