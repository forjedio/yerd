<script setup lang="ts">
import { CheckCircle2, Copy, RefreshCw, Wrench } from "lucide-vue-next";
import { computed, onMounted, ref } from "vue";

import EnvironmentCard from "@/components/EnvironmentCard.vue";
import PageHeader from "@/components/PageHeader.vue";
import Badge from "@/components/ui/Badge.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import { diagnose, doctorFix, IpcError } from "@/ipc/client";
import type { Diagnosis, Severity } from "@/ipc/types";

const toast = useToast();
const { refresh: refreshStatus } = useDaemon();

const diagnoses = ref<Diagnosis[]>([]);
const diagLoading = ref(true);
const diagError = ref(false);
const fixing = ref(false);

// "Run safe fixes" is only enabled when at least one finding is a warning/failure.
const hasActionable = computed(() =>
  diagnoses.value.some((d) => d.severity === "warn" || d.severity === "fail"),
);

const sevVariant: Record<Severity, "success" | "warning" | "destructive"> = {
  ok: "success",
  warn: "warning",
  fail: "destructive",
};

// Human labels — the wire uses bare enum tokens (ok/warn/fail) that read as
// unfinished in the UI.
const sevLabel: Record<Severity, string> = {
  ok: "Healthy",
  warn: "Warning",
  fail: "Problem",
};

// Nothing to fix: either no findings, or every finding is informational/ok.
// Show positive confirmation instead of a bare list (or a blank card).
const allClear = computed(
  () =>
    diagnoses.value.length === 0 ||
    diagnoses.value.every((d) => d.severity === "ok"),
);

async function loadDiagnoses(notify = false): Promise<void> {
  diagLoading.value = true;
  diagError.value = false;
  try {
    diagnoses.value = await diagnose();
    if (notify) toast.success("Health re-checked");
  } catch (e) {
    diagError.value = true;
    toast.error("Couldn't run diagnostics", (e as IpcError).message);
  } finally {
    diagLoading.value = false;
  }
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
  try {
    await navigator.clipboard.writeText(text);
    toast.info("Copied to clipboard");
  } catch {
    toast.error("Couldn't copy");
  }
}

onMounted(() => void loadDiagnoses());
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <PageHeader title="Doctor" subtitle="Health checks and safe one-click fixes" />

    <div class="min-h-0 flex-1 space-y-6 overflow-y-auto p-6">
      <Card>
        <CardHeader class="flex-row items-center justify-between space-y-0">
          <div class="space-y-1.5">
            <CardTitle>Health</CardTitle>
            <CardDescription>Common problems and safe one-click fixes.</CardDescription>
          </div>
          <div class="flex items-center gap-2">
            <Button
              variant="ghost"
              size="icon"
              :disabled="diagLoading"
              aria-label="Re-check health"
              @click="loadDiagnoses(true)"
            >
              <Spinner v-if="diagLoading" class="size-4" />
              <RefreshCw v-else class="size-4" />
            </Button>
            <Button size="sm" :disabled="!hasActionable || fixing" @click="runFixes">
              <Spinner v-if="fixing" class="size-4" />
              <Wrench v-else class="size-4" /> Run safe fixes
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          <div v-if="diagLoading" class="flex justify-center py-8"><Spinner class="size-5" /></div>
          <div
            v-else-if="diagError"
            class="flex flex-col items-center gap-2 py-10 text-center"
          >
            <p class="text-sm font-medium">Health check unavailable</p>
            <p class="text-sm text-muted-foreground">
              Couldn't fetch diagnostics from the daemon.
            </p>
          </div>
          <div
            v-else-if="allClear"
            class="flex flex-col items-center gap-2 py-10 text-center"
          >
            <CheckCircle2 class="size-8 text-success" />
            <div>
              <p class="text-sm font-medium">No problems found</p>
              <p class="text-sm text-muted-foreground">
                Your Yerd environment looks healthy.
              </p>
            </div>
          </div>
          <ul v-else class="space-y-3">
            <li
              v-for="(d, i) in diagnoses"
              :key="i"
              class="flex items-start gap-3 rounded-md border p-3"
            >
              <Badge :variant="sevVariant[d.severity]" class="mt-0.5 shrink-0">{{ sevLabel[d.severity] }}</Badge>
              <div class="min-w-0 flex-1">
                <p class="text-sm font-medium">{{ d.title }}</p>
                <p class="text-xs text-muted-foreground">{{ d.detail }}</p>
                <div
                  v-if="d.remedy"
                  class="mt-2 flex items-center gap-2 rounded bg-muted px-2 py-1 font-mono text-xs"
                >
                  <span class="min-w-0 flex-1 truncate">{{ d.remedy }}</span>
                  <button class="text-muted-foreground hover:text-foreground" @click="copyRemedy(d.remedy!)">
                    <Copy class="size-3.5" />
                  </button>
                </div>
              </div>
            </li>
          </ul>
        </CardContent>
      </Card>

      <!-- OS-level privileges (CA trust, .test resolver, privileged ports).
           Re-run the health checks after any elevation so the table above
           reflects the new state without a manual re-check. -->
      <EnvironmentCard @elevated="loadDiagnoses()" />
    </div>
  </div>
</template>
