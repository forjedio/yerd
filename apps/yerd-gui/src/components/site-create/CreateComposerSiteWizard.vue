<script setup lang="ts">
import { computed, nextTick, onUnmounted, reactive, ref, watch } from "vue";
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
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Select from "@/components/ui/Select.vue";
import Switch from "@/components/ui/Switch.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { comparePhpVersions } from "@/lib/phpVersion";
import { siteUrl } from "@/lib/siteUrl";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  availablePhp,
  createSite,
  installPhpWithProgress,
  installToolStreamed,
  IpcError,
  jobCancel,
  jobStatus,
  listTools,
  openInBrowser,
  openPath,
  pickDirectory,
  pollJobToEnd,
} from "@/ipc/client";
import type {
  ComposerFramework,
  CreateSiteSpec,
  JobState,
  StatusReport,
  ToolStatus,
} from "@/ipc/types";

const props = defineProps<{
  open: boolean;
  framework: ComposerFramework;
  parkedFolders: string[];
  phpVersions: string[];
  defaultPhp: string;
  tld: string;
  report: StatusReport | null;
}>();
const emit = defineEmits<{
  (e: "update:open", v: boolean): void;
  (e: "created"): void;
}>();

const toast = useToast();
const { refresh } = useDaemon();

const FRAMEWORKS: Record<ComposerFramework, { label: string; pkg: string; minPhp: string }> = {
  codeigniter: { label: "CodeIgniter", pkg: "codeigniter4/appstarter", minPhp: "8.1" },
  cakephp: { label: "CakePHP", pkg: "cakephp/app", minPhp: "8.1" },
  slim: { label: "Slim", pkg: "slim/skeleton", minPhp: "7.4" },
};
const meta = computed(() => FRAMEWORKS[props.framework]);

type Step = 0 | 1 | 2; // Basics, Review, Progress
const step = ref<Step>(0);
const STEP_LABELS = ["Basics", "Review"];

const form = reactive({
  name: "",
  location: "",
  php: "",
  secure: false,
});

const supportedPhpVersions = computed(() =>
  props.phpVersions.filter((v) => comparePhpVersions(v, meta.value.minPhp) >= 0),
);
const phpOptions = computed(() =>
  supportedPhpVersions.value.map((v) => ({ value: v, label: `PHP ${v}` })),
);

function preferredPhp(): string {
  const supported = supportedPhpVersions.value;
  if (props.defaultPhp && supported.includes(props.defaultPhp)) return props.defaultPhp;
  return supported[supported.length - 1] ?? "";
}
const locationOptions = computed(() => {
  const opts = props.parkedFolders.map((f) => ({ value: f, label: `${f}  (parked)` }));
  if (form.location && !props.parkedFolders.includes(form.location)) {
    opts.unshift({ value: form.location, label: form.location });
  }
  return opts;
});

const siteName = computed(() => form.name.trim().toLowerCase());
const nameValid = computed(() => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(siteName.value));
const projectPath = computed(() => (form.location ? `${form.location}/${siteName.value}` : ""));
const domain = computed(() => `${siteName.value || "name"}.${props.tld}`);
const openUrl = computed(() =>
  siteUrl({ name: siteName.value || "name", secure: form.secure }, props.report),
);

const basicsValid = computed(
  () => nameValid.value && form.location.trim() !== "" && form.php !== "",
);

// ── prerequisites ────────────────────────────────────────────────────────────
const tools = ref<ToolStatus[]>([]);
const toolsLoading = ref(false);
const installingTool = ref<string | null>(null);
const installingAll = ref(false);
const installLog = ref<string[]>([]);
const installLogBox = ref<HTMLElement | null>(null);

async function appendInstallLog(lines: string[]): Promise<void> {
  installLog.value.push(...lines);
  await nextTick();
  const el = installLogBox.value;
  if (el) el.scrollTop = el.scrollHeight;
}

const needsComposer = computed(
  () => !tools.value.some((t) => t.id === "composer" && (t.installed || t.external)),
);
const noSupportedPhp = computed(() => supportedPhpVersions.value.length === 0);
const phpUnsupported = computed(() => noSupportedPhp.value && props.phpVersions.length > 0);

const ready = computed(() => !noSupportedPhp.value && !needsComposer.value);
const installBusy = computed(() => installingAll.value || installingTool.value !== null);

async function refreshTools(): Promise<void> {
  toolsLoading.value = true;
  try {
    tools.value = await listTools();
  } catch {
    tools.value = [];
  } finally {
    toolsLoading.value = false;
  }
}

async function installComposer(): Promise<boolean> {
  installingTool.value = "composer";
  installLog.value = [];
  try {
    const jobId = await installToolStreamed("composer");
    const final = await pollJobToEnd(
      jobId,
      (lines) => void appendInstallLog(lines),
      () => props.open,
    );
    await refreshTools();
    if (final.state === "running") return false;
    if (final.state !== "succeeded") {
      toast.error("Couldn't install composer", final.error ?? "install failed");
      return false;
    }
    return true;
  } catch (e) {
    toast.error("Couldn't install composer", (e as IpcError).message);
    return false;
  } finally {
    installingTool.value = null;
  }
}

async function installFirstPhp(): Promise<boolean> {
  installingTool.value = "php";
  installLog.value = [];
  try {
    const { available } = await availablePhp();
    const supported = available.filter((v) => comparePhpVersions(v, meta.value.minPhp) >= 0);
    const version = supported[supported.length - 1];
    if (!version) {
      toast.error(
        "Couldn't install PHP",
        `No installable PHP ${meta.value.minPhp} or newer was found.`,
      );
      return false;
    }
    void appendInstallLog([`Installing PHP ${version}…`]);
    await installPhpWithProgress(version, (lines) => void appendInstallLog(lines));
    await refresh();
    void appendInstallLog([`Installed PHP ${version}`]);
    return true;
  } catch (e) {
    toast.error("Couldn't install PHP", (e as IpcError).message);
    return false;
  } finally {
    installingTool.value = null;
  }
}

async function installAllMissing(): Promise<void> {
  installingAll.value = true;
  try {
    if (noSupportedPhp.value && !(await installFirstPhp())) return;
    if (needsComposer.value && !(await installComposer())) return;
    await nextTick();
    toast.success("Toolchain ready");
  } finally {
    installingAll.value = false;
  }
}

function buildSpec(): CreateSiteSpec {
  return {
    name: form.name.trim(),
    parent_dir: form.location,
    php: form.php,
    secure: form.secure,
    framework: { framework: props.framework },
  };
}

// ── progress / job polling ───────────────────────────────────────────────────
const jobId = ref<string | null>(null);
const jobStateRef = ref<JobState>("running");
const phase = ref("Starting");
const log = ref<string[]>([]);
const jobError = ref<string | null>(null);
const logBox = ref<HTMLElement | null>(null);
let cursor = 0;
let pollTimer: number | null = null;

const PHASES = ["Preflight", "Scaffolding", "Registering", "Done"];
function phaseStatus(p: string): "done" | "active" | "todo" {
  const ci = PHASES.indexOf(phase.value);
  const pi = PHASES.indexOf(p);
  if (jobStateRef.value === "succeeded") return "done";
  if (ci === -1) return p === "Preflight" ? "active" : "todo";
  if (pi < ci) return "done";
  if (pi === ci) return "active";
  return "todo";
}

async function chooseLocation(): Promise<void> {
  const dir = await pickDirectory(form.location || undefined);
  if (dir) form.location = dir;
}

async function startCreate(): Promise<void> {
  step.value = 2;
  jobStateRef.value = "running";
  phase.value = "Starting";
  log.value = [];
  jobError.value = null;
  cancelRequested.value = false;
  cursor = 0;
  try {
    jobId.value = await createSite(buildSpec());
    poll();
  } catch (e) {
    jobStateRef.value = "failed";
    jobError.value = (e as IpcError).message;
  }
}

function poll(): void {
  if (!jobId.value) return;
  void (async () => {
    try {
      const r = await jobStatus(jobId.value as string, cursor);
      if (r.log.length) {
        log.value.push(...r.log);
        void scrollLog();
      }
      cursor = r.next_cursor;
      phase.value = r.phase;
      jobStateRef.value = r.state;
      jobError.value = r.error;
      if (r.state === "running") {
        pollTimer = window.setTimeout(poll, 600);
        return;
      }
      if (r.state === "succeeded") emit("created");
      if (!props.open) notifyFinishedInBackground(r.state, r.error);
    } catch (e) {
      jobStateRef.value = "failed";
      jobError.value = (e as IpcError).message;
    }
  })();
}

function notifyFinishedInBackground(state: JobState, error: string | null): void {
  if (state === "succeeded") toast.success(`${domain.value} is ready`);
  else if (state === "failed") toast.error(`Couldn't create ${domain.value}`, error ?? "creation failed");
}

async function scrollLog(): Promise<void> {
  await nextTick();
  const el = logBox.value;
  if (el) el.scrollTop = el.scrollHeight;
}

const cancelRequested = ref(false);
async function cancelJob(): Promise<void> {
  if (!jobId.value || cancelRequested.value) return;
  cancelRequested.value = true;
  try {
    await jobCancel(jobId.value);
  } catch {
    /* the job may already be finishing; ignore */
  }
}

function stopPolling(): void {
  if (pollTimer !== null) {
    window.clearTimeout(pollTimer);
    pollTimer = null;
  }
}

// ── lifecycle ────────────────────────────────────────────────────────────────
// A create keeps polling while the dialog is closed, so the site list refreshes
// when it lands; reopening mid-create resumes its progress view.
function jobInFlight(): boolean {
  return jobId.value !== null && jobStateRef.value === "running";
}

function resetForm(): void {
  step.value = 0;
  form.name = "";
  form.location = props.parkedFolders[0] ?? "";
  form.php = preferredPhp();
  form.secure = false;
  jobId.value = null;
  jobError.value = null;
  log.value = [];
  installLog.value = [];
  jobStateRef.value = "running";
  phase.value = "Starting";
  cursor = 0;
  installingTool.value = null;
  installingAll.value = false;
  cancelRequested.value = false;
}

watch(
  () => props.open,
  (open) => {
    if (open && !jobInFlight()) {
      resetForm();
      void refreshTools();
    }
  },
  { immediate: true },
);

watch(
  () => props.phpVersions,
  () => {
    if (!form.php || !supportedPhpVersions.value.includes(form.php)) {
      form.php = preferredPhp();
    }
  },
);

onUnmounted(stopPolling);

const busy = computed(() => jobStateRef.value === "running" && step.value === 2);
</script>

<template>
  <Modal
    :open="open"
    :title="`Create a new ${meta.label} site`"
    size="lg"
    @update:open="(v) => emit('update:open', v)"
  >
    <!-- checking the toolchain -->
    <div v-if="toolsLoading" class="flex items-center justify-center py-12">
      <Spinner class="size-6" />
    </div>

    <!-- ── Prerequisites gate ── -->
    <div v-else-if="!ready" class="space-y-4">
      <div class="flex items-start gap-2 rounded-lg border border-warning/40 bg-warning/10 p-3">
        <TriangleAlert class="mt-0.5 size-4 shrink-0 text-warning" />
        <div>
          <p class="text-sm font-medium">A few tools are needed first</p>
          <p class="text-xs text-muted-foreground">
            Creating a {{ meta.label }} site needs PHP and Composer. Install the missing ones to
            continue.
          </p>
        </div>
      </div>

      <div class="divide-y rounded-lg border">
        <div
          v-for="row in [
            { id: 'php', label: 'PHP', sub: phpUnsupported ? `Installed, but not ${meta.minPhp}+` : `Runtime ${meta.minPhp}+`, ok: !noSupportedPhp },
            { id: 'composer', label: 'Composer', sub: 'Dependency manager', ok: !needsComposer },
          ]"
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
              <Spinner v-if="installingTool === row.id" class="size-4" />
              <Button
                v-else
                size="sm"
                variant="outline"
                :disabled="installBusy"
                @click="row.id === 'php' ? installFirstPhp() : installComposer()"
              >
                Install
              </Button>
            </template>
          </div>
        </div>
      </div>

      <pre
        v-if="installLog.length"
        ref="installLogBox"
        class="h-40 overflow-y-auto whitespace-pre-wrap rounded-lg bg-zinc-950 p-3 font-mono text-[11px] leading-relaxed text-zinc-200"
      >{{ installLog.join("\n") }}</pre>
    </div>

    <!-- ── Wizard ── -->
    <template v-else>
    <div v-if="step < 2" class="mb-6 flex w-full">
      <div
        v-for="(label, i) in STEP_LABELS"
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

    <!-- ── Step 1: Basics ── -->
    <div v-if="step === 0" class="space-y-4">
      <div>
        <label class="text-sm font-medium" for="ccs-name">Project name</label>
        <Input id="ccs-name" v-model="form.name" placeholder="e.g. blog" class="mt-2" />
        <p class="mt-1 text-xs text-muted-foreground">
          Served at
          <span class="font-mono text-foreground">{{ domain }}</span>
          <span v-if="projectPath"> · creates <span class="font-mono">{{ projectPath }}</span></span>
        </p>
        <p v-if="form.name && !nameValid" class="mt-1 text-xs text-destructive">
          Use a single label: letters, numbers and hyphens only.
        </p>
      </div>

      <div>
        <label class="text-sm font-medium" for="ccs-location">Location</label>
        <div class="mt-2 flex gap-2">
          <Select
            v-if="locationOptions.length"
            id="ccs-location"
            :model-value="form.location"
            :options="locationOptions"
            class="w-full"
            aria-label="Location"
            @update:model-value="(v: string) => (form.location = v)"
          />
          <Input v-else :model-value="form.location" readonly placeholder="Choose a folder…" />
          <Button variant="outline" @click="chooseLocation">
            <FolderOpen class="size-4" /> Browse
          </Button>
        </div>
        <p class="mt-1 text-xs text-muted-foreground">
          A parked folder serves the new site automatically; any other folder is linked.
        </p>
      </div>

      <div class="flex items-center justify-between gap-4 rounded-lg border p-3">
        <div>
          <p class="text-sm font-medium">PHP version</p>
          <p class="text-xs text-muted-foreground">
            The version this site runs on. {{ meta.label }} needs PHP {{ meta.minPhp }} or newer.
          </p>
        </div>
        <Select
          v-if="phpOptions.length"
          id="ccs-php"
          :model-value="form.php"
          :options="phpOptions"
          class="w-40 shrink-0"
          aria-label="PHP version"
          @update:model-value="(v: string) => (form.php = v)"
        />
        <span v-else class="shrink-0 text-xs text-destructive">
          No supported PHP installed.
        </span>
      </div>

      <div class="flex items-center justify-between gap-4 rounded-lg border p-3">
        <div>
          <p class="text-sm font-medium">HTTPS</p>
          <p class="text-xs text-muted-foreground">Serve this site over TLS.</p>
        </div>
        <Switch v-model="form.secure" aria-label="Serve over HTTPS" />
      </div>
    </div>

    <!-- ── Step 2: Review ── -->
    <div v-else-if="step === 1" class="space-y-4">
      <div class="rounded-lg border p-3 text-sm">
        <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5">
          <dt class="text-muted-foreground">Site</dt>
          <dd class="font-mono">{{ domain }}</dd>
          <dt class="text-muted-foreground">Path</dt>
          <dd class="truncate font-mono">{{ projectPath }}</dd>
          <dt class="text-muted-foreground">PHP</dt>
          <dd>{{ form.php }}{{ form.secure ? " · HTTPS" : "" }}</dd>
          <dt class="text-muted-foreground">Template</dt>
          <dd class="font-mono">{{ meta.pkg }}</dd>
        </dl>
      </div>
    </div>

    <!-- ── Step 3: Progress ── -->
    <div v-else class="space-y-4">
      <div class="flex items-center">
        <template v-for="(p, i) in PHASES" :key="p">
          <div class="flex shrink-0 items-center gap-2">
            <span
              class="flex size-6 items-center justify-center rounded-full transition-colors"
              :class="
                phaseStatus(p) === 'done'
                  ? 'bg-success text-white'
                  : phaseStatus(p) === 'active'
                    ? 'bg-brand text-white'
                    : 'bg-muted text-muted-foreground'
              "
            >
              <Check v-if="phaseStatus(p) === 'done'" class="size-3.5" />
              <Loader2 v-else-if="phaseStatus(p) === 'active'" class="size-3.5 animate-spin" />
              <Circle v-else class="size-2 fill-current" />
            </span>
            <span
              class="text-xs"
              :class="phaseStatus(p) === 'todo' ? 'text-muted-foreground' : 'font-medium'"
            >{{ p }}</span>
          </div>
          <span
            v-if="i < PHASES.length - 1"
            class="mx-2 h-0.5 flex-1 rounded-full transition-colors"
            :class="phaseStatus(p) === 'done' ? 'bg-success' : 'bg-border'"
          />
        </template>
      </div>

      <pre
        ref="logBox"
        class="h-56 overflow-y-auto whitespace-pre-wrap rounded-lg bg-zinc-950 p-3 font-mono text-[11px] leading-relaxed text-zinc-200"
      >{{ log.join("\n") || "Starting…" }}</pre>

      <div
        v-if="jobStateRef === 'succeeded'"
        class="flex items-center gap-2 rounded-lg border border-success/40 bg-success/10 p-3 text-sm text-success"
      >
        <CheckCircle2 class="size-4" /> {{ domain }} is ready.
      </div>
      <div
        v-else-if="jobStateRef === 'failed'"
        class="rounded-lg border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive"
      >
        {{ jobError || "Creation failed." }}
      </div>
      <div
        v-else-if="jobStateRef === 'cancelled'"
        class="rounded-lg border p-3 text-sm text-muted-foreground"
      >
        Cancelled.
      </div>
    </div>
    </template>

    <!-- ── footer ── -->
    <template #footer="{ close: modalClose }">
      <template v-if="toolsLoading">
        <Button variant="ghost" @click="modalClose">Cancel</Button>
      </template>

      <template v-else-if="!ready">
        <Button variant="ghost" @click="modalClose">Cancel</Button>
        <Button :disabled="installBusy" @click="installAllMissing">
          <Spinner v-if="installingAll" class="size-4" /> Install missing tools
        </Button>
      </template>

      <template v-else-if="step === 2">
        <template v-if="busy">
          <Button variant="ghost" :disabled="cancelRequested" @click="cancelJob">
            {{ cancelRequested ? "Cancelling…" : "Cancel" }}
          </Button>
        </template>
        <template v-else-if="jobStateRef === 'succeeded'">
          <Button variant="outline" @click="openPath(projectPath)">
            <FolderOpen class="size-4" /> Open folder
          </Button>
          <Button variant="outline" @click="openInBrowser(openUrl)">
            <ExternalLink class="size-4" /> Open in browser
          </Button>
          <Button @click="modalClose">Done</Button>
        </template>
        <template v-else>
          <Button variant="ghost" @click="step = 1">
            <ChevronLeft class="size-4" /> Back
          </Button>
          <Button @click="modalClose">Close</Button>
        </template>
      </template>

      <template v-else>
        <Button v-if="step > 0" variant="ghost" @click="step = 0">
          <ChevronLeft class="size-4" /> Back
        </Button>
        <Button v-else variant="ghost" @click="modalClose">Cancel</Button>

        <Button v-if="step === 0" :disabled="!basicsValid" @click="step = 1">Next</Button>
        <Button v-else :disabled="!ready" @click="startCreate">Create site</Button>
      </template>
    </template>
  </Modal>
</template>

<style scoped>
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
.step-chevron-sep {
  filter: drop-shadow(-2.5px 0 0 #a1a1aa);
}
</style>
