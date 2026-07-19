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
import { phpVersionInRange } from "@/lib/phpVersion";
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
  AuthProvider,
  CreateSiteSpec,
  Database,
  JobState,
  JsRuntime,
  LaravelOptions,
  StarterKit,
  StarterKitTag,
  StatusReport,
  Testing,
  ToolStatus,
} from "@/ipc/types";

const props = defineProps<{
  open: boolean;
  parkedFolders: string[];
  phpVersions: string[];
  defaultPhp: string;
  tld: string;
  /** Live daemon status, used to build the post-create "Open" URL so it matches
   *  the rest of the GUI (resolver-on `.test` + bound port vs localhost `/~`). */
  report: StatusReport | null;
}>();
const emit = defineEmits<{
  (e: "update:open", v: boolean): void;
  (e: "created"): void;
}>();

const toast = useToast();
const { refresh } = useDaemon();

// ── wizard state ─────────────────────────────────────────────────────────────
type Step = 0 | 1 | 2 | 3 | 4; // Basics, Stack, Testing, Review, Progress
const step = ref<Step>(0);

const STEP_LABELS = ["Basics", "Stack", "Testing", "Review"];

const form = reactive({
  name: "",
  location: "",
  php: "",
  secure: false,
  // stack
  communityPackage: "",
  auth: "laravel" as AuthProvider,
  livewireClassComponents: false,
  teams: false,
  js: "npm" as JsRuntime,
  // testing & data
  testing: "pest" as Testing,
  database: "sqlite" as Database,
  git: true,
  boost: false,
});

const KIT_OPTIONS: { value: StarterKitTag | "community"; label: string; hint: string }[] = [
  { value: "none", label: "None", hint: "Plain skeleton" },
  { value: "react", label: "React", hint: "Inertia + TS" },
  { value: "vue", label: "Vue", hint: "Inertia + TS" },
  { value: "livewire", label: "Livewire", hint: "Blade + PHP" },
  { value: "svelte", label: "Svelte", hint: "Inertia + TS" },
  { value: "community", label: "Community…", hint: "--using <package>" },
];

const kitChoice = ref<StarterKitTag | "community">("none");
const isJsKit = computed(() =>
  ["react", "vue", "svelte"].includes(kitChoice.value) ||
  (kitChoice.value === "community"),
);
const isLivewire = computed(() => kitChoice.value === "livewire");
const hasAuthKit = computed(() => kitChoice.value !== "none");

// The Laravel installer only supports this window; anything outside it is hidden
// from the picker rather than offered and then failing mid-create.
const LARAVEL_MIN_PHP = "8.3";
const LARAVEL_MAX_PHP = "8.5";
const supportedPhpVersions = computed(() =>
  props.phpVersions.filter((v) => phpVersionInRange(v, LARAVEL_MIN_PHP, LARAVEL_MAX_PHP)),
);
const phpOptions = computed(() =>
  supportedPhpVersions.value.map((v) => ({ value: v, label: `PHP ${v}` })),
);

// The default only applies when it falls in the supported window; otherwise the
// newest supported version wins so the Basics step always opens on a valid pick.
function preferredPhp(): string {
  const supported = supportedPhpVersions.value;
  if (props.defaultPhp && supported.includes(props.defaultPhp)) return props.defaultPhp;
  return supported[supported.length - 1] ?? "";
}
const locationOptions = computed(() => {
  const opts = props.parkedFolders.map((f) => ({ value: f, label: `${f}  (parked)` }));
  // Include a custom-picked location that isn't a parked root.
  if (form.location && !props.parkedFolders.includes(form.location)) {
    opts.unshift({ value: form.location, label: form.location });
  }
  return opts;
});

const nameValid = computed(() => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i.test(form.name.trim()));
const projectPath = computed(() =>
  form.location ? `${form.location}/${form.name.trim()}` : "",
);
const domain = computed(() => `${form.name.trim().toLowerCase() || "name"}.${props.tld}`);
// Browser URL for the finished site's "Open" action. Reuse the shared helper so
// it matches the rest of the GUI exactly: resolver-on uses the `.test` domain
// with the correct scheme + bound port (honouring port-redirect), resolver-off
// uses the http://localhost/~ fallback.
const openUrl = computed(() =>
  siteUrl({ name: form.name.trim().toLowerCase() || "name", secure: form.secure }, props.report),
);

const basicsValid = computed(
  () => nameValid.value && form.location.trim() !== "" && form.php !== "",
);
const stackValid = computed(
  () => kitChoice.value !== "community" || form.communityPackage.trim() !== "",
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

// A tool satisfies a prerequisite if it's Yerd-managed OR externally available
// on the user's PATH - the scaffold can use either.
function toolAvailable(id: string): boolean {
  return tools.value.some((t) => t.id === id && (t.installed || t.external));
}
// Managed-only: building the *managed* Laravel installer requires Yerd's own
// Composer (an external Composer can't build it). Gates the laravel install step.
const managedComposer = computed(() =>
  tools.value.some((t) => t.id === "composer" && t.installed),
);
// PHP, Composer and the Laravel installer are *required* and gated up front.
const needsComposer = computed(() => !toolAvailable("composer"));
const needsInstaller = computed(() => !toolAvailable("laravel"));
const noSupportedPhp = computed(() => supportedPhpVersions.value.length === 0);
// Distinguishes "no PHP at all" from "PHP installed, none of it supported" so
// the prerequisites row can say which, rather than implying PHP is missing.
const phpUnsupported = computed(() => noSupportedPhp.value && props.phpVersions.length > 0);
// Node/Bun are only conditionally needed and the daemon installs them inline
// during the job (shown as a phase), so they don't block the wizard.
const needsNode = computed(() => form.js === "npm" && !toolAvailable("node"));
const needsBun = computed(() => form.js === "bun" && !toolAvailable("bun"));

/** Whether the required toolchain is present (the wizard is unlocked). */
const ready = computed(
  () => !noSupportedPhp.value && !needsComposer.value && !needsInstaller.value,
);
/** Any prerequisite install in flight (per-item or install-all). */
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

async function installPrereq(id: "composer" | "laravel" | "node" | "bun"): Promise<boolean> {
  installingTool.value = id;
  installLog.value = [];
  try {
    const jobId = await installToolStreamed(id);
    const final = await pollJobToEnd(
      jobId,
      (lines) => void appendInstallLog(lines),
      () => props.open,
    );
    await refreshTools();
    // "running" means the poll stopped because the wizard closed, not a failure.
    if (final.state === "running") return false;
    if (final.state !== "succeeded") {
      toast.error(`Couldn't install ${id}`, final.error ?? "install failed");
      return false;
    }
    return true;
  } catch (e) {
    toast.error(`Couldn't install ${id}`, (e as IpcError).message);
    return false;
  } finally {
    installingTool.value = null;
  }
}

/**
 * Install the newest available minor within the supported window (the
 * distribution returns them ascending), rather than pinning a version that rots
 * each release. The daemon resolves the patch and makes it the global default;
 * the live status poll then surfaces it, which unlocks the wizard automatically.
 */
async function installFirstPhp(): Promise<boolean> {
  installingTool.value = "php";
  installLog.value = [];
  try {
    const { available } = await availablePhp();
    const supported = available.filter((v) =>
      phpVersionInRange(v, LARAVEL_MIN_PHP, LARAVEL_MAX_PHP),
    );
    const version = supported[supported.length - 1];
    if (!version) {
      toast.error(
        "Couldn't install PHP",
        `No installable PHP between ${LARAVEL_MIN_PHP} and ${LARAVEL_MAX_PHP} was found.`,
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

/** Install every missing required tool, in dependency order, then unlock. */
async function installAllMissing(): Promise<void> {
  installingAll.value = true;
  try {
    if (noSupportedPhp.value && !(await installFirstPhp())) return;
    if (needsComposer.value && !(await installPrereq("composer"))) return;
    // The managed Laravel installer is BUILT via Yerd's own Composer, so it can
    // only be auto-installed when managed Composer is present. If Composer is only
    // external, skip (the daemon would reject it) and guide the user.
    if (needsInstaller.value) {
      if (!managedComposer.value) {
        toast.error(
          "Can't install the Laravel installer",
          "Yerd needs its own Composer to build it - install Yerd's Composer, or run `composer global require laravel/installer`.",
        );
        return;
      }
      if (!(await installPrereq("laravel"))) return;
    }
    // Reaching here means every missing tool installed successfully. Don't gate
    // the toast on `ready` - it derives from the async `phpVersions` prop, which
    // may not have propagated yet.
    await nextTick();
    toast.success("Toolchain ready");
  } finally {
    installingAll.value = false;
  }
}

function buildSpec(): CreateSiteSpec {
  const kit: StarterKit =
    kitChoice.value === "community"
      ? { community: form.communityPackage.trim() }
      : (kitChoice.value as StarterKitTag);
  const options: LaravelOptions = {
    starter_kit: kit,
    auth: hasAuthKit.value ? form.auth : "laravel",
    livewire_class_components: isLivewire.value && form.livewireClassComponents,
    teams: hasAuthKit.value && form.teams,
    testing: form.testing,
    database: form.database,
    // The JS runtime selector only appears for Inertia kits; for None/Livewire
    // never trigger an npm/Node install the user didn't ask for.
    js: isJsKit.value ? form.js : "skip",
    git: form.git,
    boost: form.boost,
  };
  return {
    name: form.name.trim(),
    parent_dir: form.location,
    php: form.php,
    secure: form.secure,
    framework: { framework: "laravel", options },
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
  const cur = phase.value;
  const ci = PHASES.indexOf(cur);
  const pi = PHASES.indexOf(p);
  if (jobStateRef.value === "succeeded") return "done";
  if (ci === -1) return p === "Preflight" ? "active" : "todo"; // e.g. "Installing Node"
  if (pi < ci) return "done";
  if (pi === ci) return "active";
  return "todo";
}

async function chooseLocation(): Promise<void> {
  const dir = await pickDirectory(form.location || undefined);
  if (dir) form.location = dir;
}

async function startCreate(): Promise<void> {
  step.value = 4;
  jobStateRef.value = "running";
  phase.value = "Starting";
  log.value = [];
  jobError.value = null;
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
      cursor = r.next_cursor; // advance unconditionally (phase-only ticks too)
      phase.value = r.phase;
      jobStateRef.value = r.state;
      jobError.value = r.error;
      // Stop if the modal was closed mid-poll (don't re-arm into the background).
      if (r.state === "running" && props.open) {
        pollTimer = window.setTimeout(poll, 600);
      } else if (r.state === "succeeded") {
        emit("created");
      }
    } catch (e) {
      jobStateRef.value = "failed";
      jobError.value = (e as IpcError).message;
    }
  })();
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
function resetForm(): void {
  step.value = 0;
  form.name = "";
  form.location = props.parkedFolders[0] ?? "";
  form.php = preferredPhp();
  form.secure = false;
  kitChoice.value = "none";
  form.communityPackage = "";
  form.auth = "laravel";
  form.livewireClassComponents = false;
  form.teams = false;
  form.js = "npm";
  form.testing = "pest";
  form.database = "sqlite";
  form.git = true;
  form.boost = false;
  // progress / install state
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
    if (open) {
      resetForm();
      void refreshTools();
    } else {
      stopPolling();
    }
  },
);

// Keep the selection honest as the installed set changes underneath the wizard
// (PHP installed from the prerequisites screen, or the selected version
// uninstalled elsewhere): anything no longer supported falls back to the
// preferred version rather than being submitted for a version that isn't there.
watch(
  () => props.phpVersions,
  () => {
    if (!form.php || !supportedPhpVersions.value.includes(form.php)) {
      form.php = preferredPhp();
    }
  },
);

// Stop the create-job poll if the component is ever torn down mid-flight. The
// streamed install polls (pollJobToEnd) self-stop via their `() => props.open`
// predicate.
onUnmounted(stopPolling);

const busy = computed(() => jobStateRef.value === "running" && step.value === 4);
</script>

<template>
  <Modal
    :open="open"
    title="Create a new Laravel site"
    size="lg"
    @update:open="(v) => emit('update:open', v)"
  >
    <!-- checking the toolchain -->
    <div v-if="toolsLoading" class="flex items-center justify-center py-12">
      <Spinner class="size-6" />
    </div>

    <!-- ── Prerequisites gate (first page when tooling is missing) ── -->
    <div v-else-if="!ready" class="space-y-4">
      <div class="flex items-start gap-2 rounded-lg border border-warning/40 bg-warning/10 p-3">
        <TriangleAlert class="mt-0.5 size-4 shrink-0 text-warning" />
        <div>
          <p class="text-sm font-medium">A few tools are needed first</p>
          <p class="text-xs text-muted-foreground">
            Creating a Laravel site needs PHP, Composer and the Laravel installer.
            Install the missing ones to continue.
          </p>
        </div>
      </div>

      <div class="divide-y rounded-lg border">
        <div
          v-for="row in [
            { id: 'php', label: 'PHP', sub: phpUnsupported ? `Installed, but not ${LARAVEL_MIN_PHP}-${LARAVEL_MAX_PHP}` : `Runtime ${LARAVEL_MIN_PHP}-${LARAVEL_MAX_PHP}`, ok: !noSupportedPhp, busyKey: 'php' },
            { id: 'composer', label: 'Composer', sub: 'Dependency manager', ok: !needsComposer, busyKey: 'composer' },
            { id: 'laravel', label: 'Laravel installer', sub: 'laravel new', ok: !needsInstaller, busyKey: 'laravel' },
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
              <Spinner v-if="installingTool === row.busyKey" class="size-4" />
              <Button
                v-else
                size="sm"
                variant="outline"
                :disabled="installBusy || (row.id === 'laravel' && !managedComposer)"
                :title="row.id === 'laravel' && !managedComposer ? 'Yerd\'s own Composer is required to build the Laravel installer' : ''"
                @click="row.id === 'php' ? installFirstPhp() : installPrereq(row.id as 'composer' | 'laravel')"
              >
                Install
              </Button>
            </template>
          </div>
        </div>
      </div>

      <!-- live install output -->
      <pre
        v-if="installLog.length"
        ref="installLogBox"
        class="h-40 overflow-y-auto whitespace-pre-wrap rounded-lg bg-zinc-950 p-3 font-mono text-[11px] leading-relaxed text-zinc-200"
      >{{ installLog.join("\n") }}</pre>
    </div>

    <!-- ── Wizard (unlocked once the toolchain is present) ── -->
    <template v-else>
    <!-- step indicator: chevron progress (4 evenly-spaced segments) -->
    <div v-if="step < 4" class="mb-6 flex w-full">
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
        <label class="text-sm font-medium" for="cs-name">Project name</label>
        <Input id="cs-name" v-model="form.name" placeholder="e.g. blog" class="mt-2" />
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
        <label class="text-sm font-medium" for="cs-location">Location</label>
        <div class="mt-2 flex gap-2">
          <Select
            v-if="locationOptions.length"
            id="cs-location"
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
            The version this site runs on. Laravel needs PHP {{ LARAVEL_MIN_PHP }} to
            {{ LARAVEL_MAX_PHP }}.
          </p>
        </div>
        <Select
          v-if="phpOptions.length"
          id="cs-php"
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

    <!-- ── Step 2: Stack ── -->
    <div v-else-if="step === 1" class="space-y-4">
      <div>
        <span class="text-sm font-medium">Starter kit</span>
        <div class="mt-2 grid grid-cols-3 gap-2">
          <button
            v-for="k in KIT_OPTIONS"
            :key="k.value"
            type="button"
            class="rounded-lg border p-2.5 text-left transition-colors"
            :class="
              kitChoice === k.value
                ? 'border-brand bg-brand/5 ring-1 ring-brand'
                : 'hover:border-brand/40'
            "
            @click="kitChoice = k.value"
          >
            <span class="block text-sm font-medium">{{ k.label }}</span>
            <span class="block text-[11px] text-muted-foreground">{{ k.hint }}</span>
          </button>
        </div>
      </div>

      <div v-if="kitChoice === 'community'">
        <label class="text-sm font-medium" for="cs-pkg">Community package</label>
        <Input
          id="cs-pkg"
          v-model="form.communityPackage"
          placeholder="vendor/starter-kit"
          class="mt-2"
        />
      </div>

      <div v-if="isJsKit" class="flex gap-4">
        <div class="flex-1">
          <label class="text-sm font-medium" for="cs-js">Frontend dependencies</label>
          <Select
            id="cs-js"
            :model-value="form.js"
            :options="[
              { value: 'npm', label: 'Install & build with npm' },
              { value: 'bun', label: 'Install & build with Bun' },
              { value: 'skip', label: 'Skip (install later)' },
            ]"
            class="mt-2 w-full"
            aria-label="Frontend dependencies"
            @update:model-value="(v: string) => (form.js = v as JsRuntime)"
          />
          <p v-if="(needsNode || needsBun)" class="mt-1 text-xs text-muted-foreground">
            {{ needsNode ? "Node" : "Bun" }} will be installed automatically.
          </p>
        </div>
        <div class="flex-1">
          <label class="text-sm font-medium" for="cs-auth">Authentication</label>
          <Select
            id="cs-auth"
            :model-value="form.auth"
            :options="[
              { value: 'laravel', label: 'Built-in (Laravel)' },
              { value: 'work_os', label: 'WorkOS AuthKit' },
            ]"
            class="mt-2 w-full"
            aria-label="Authentication provider"
            @update:model-value="(v: string) => (form.auth = v as AuthProvider)"
          />
        </div>
      </div>

      <div v-if="isLivewire" class="flex items-center justify-between gap-4">
        <div>
          <p class="text-sm font-medium">Authentication</p>
        </div>
        <Select
          :model-value="form.auth"
          :options="[
            { value: 'laravel', label: 'Built-in (Laravel)' },
            { value: 'work_os', label: 'WorkOS AuthKit' },
          ]"
          class="w-48"
          aria-label="Authentication provider"
          @update:model-value="(v: string) => (form.auth = v as AuthProvider)"
        />
      </div>

      <div v-if="hasAuthKit" class="space-y-3 rounded-lg border bg-muted/30 p-3">
        <div v-if="isLivewire" class="flex items-center justify-between gap-4">
          <span class="text-sm">Standalone Livewire class components</span>
          <Switch v-model="form.livewireClassComponents" aria-label="Livewire class components" />
        </div>
        <div class="flex items-center justify-between gap-4">
          <span class="text-sm">Team support</span>
          <Switch v-model="form.teams" aria-label="Team support" />
        </div>
      </div>

      <p v-if="kitChoice === 'none'" class="text-xs text-muted-foreground">
        No starter kit - a plain Laravel application with no auth scaffolding.
      </p>
    </div>

    <!-- ── Step 3: Testing & data ── -->
    <div v-else-if="step === 2" class="space-y-4">
      <div class="flex gap-4">
        <div class="flex-1">
          <label class="text-sm font-medium" for="cs-test">Testing framework</label>
          <Select
            id="cs-test"
            :model-value="form.testing"
            :options="[
              { value: 'pest', label: 'Pest' },
              { value: 'php_unit', label: 'PHPUnit' },
            ]"
            class="mt-2 w-full"
            aria-label="Testing framework"
            @update:model-value="(v: string) => (form.testing = v as Testing)"
          />
        </div>
        <div class="flex-1">
          <label class="text-sm font-medium" for="cs-db">Database</label>
          <Select
            id="cs-db"
            :model-value="form.database"
            :options="[
              { value: 'sqlite', label: 'SQLite' },
              { value: 'mysql', label: 'MySQL' },
              { value: 'mariadb', label: 'MariaDB' },
              { value: 'pgsql', label: 'PostgreSQL' },
              { value: 'sqlsrv', label: 'SQL Server' },
            ]"
            class="mt-2 w-full"
            aria-label="Database"
            @update:model-value="(v: string) => (form.database = v as Database)"
          />
        </div>
      </div>
      <p v-if="form.database !== 'sqlite'" class="text-xs text-muted-foreground">
        The driver is written to <code class="font-mono">.env</code>; provision the database
        yourself (live DB setup lands with services).
      </p>

      <div class="space-y-3 rounded-lg border p-3">
        <div class="flex items-center justify-between gap-4">
          <div>
            <p class="text-sm font-medium">Initialise git</p>
            <p class="text-xs text-muted-foreground">Run <code class="font-mono">git init</code> in the new project.</p>
          </div>
          <Switch v-model="form.git" aria-label="Initialise git" />
        </div>
        <div class="flex items-center justify-between gap-4">
          <div>
            <p class="text-sm font-medium">Laravel Boost</p>
            <p class="text-xs text-muted-foreground">Install Boost for AI-assisted coding.</p>
          </div>
          <Switch v-model="form.boost" aria-label="Laravel Boost" />
        </div>
      </div>
    </div>

    <!-- ── Step 4: Review ── -->
    <div v-else-if="step === 3" class="space-y-4">
      <div class="rounded-lg border p-3 text-sm">
        <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5">
          <dt class="text-muted-foreground">Site</dt>
          <dd class="font-mono">{{ domain }}</dd>
          <dt class="text-muted-foreground">Path</dt>
          <dd class="truncate font-mono">{{ projectPath }}</dd>
          <dt class="text-muted-foreground">PHP</dt>
          <dd>{{ form.php }}{{ form.secure ? " · HTTPS" : "" }}</dd>
        </dl>
      </div>

      <p v-if="needsNode || needsBun" class="text-xs text-muted-foreground">
        {{ needsNode ? "Node" : "Bun" }} will be installed automatically during creation.
      </p>
    </div>

    <!-- ── Step 5: Progress ── -->
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
      <p v-if="phase && !PHASES.includes(phase) && jobStateRef === 'running'" class="text-xs text-muted-foreground">
        {{ phase }}…
      </p>

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
      <!-- Checking toolchain -->
      <template v-if="toolsLoading">
        <Button variant="ghost" @click="modalClose">Cancel</Button>
      </template>

      <!-- Prerequisites gate: install everything missing, then unlock. -->
      <template v-else-if="!ready">
        <Button variant="ghost" @click="modalClose">Cancel</Button>
        <Button :disabled="installBusy" @click="installAllMissing">
          <Spinner v-if="installingAll" class="size-4" /> Install missing tools
        </Button>
      </template>

      <!-- Progress step has its own controls -->
      <template v-else-if="step === 4">
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
          <Button variant="ghost" @click="step = 3">
            <ChevronLeft class="size-4" /> Back
          </Button>
          <Button @click="modalClose">Close</Button>
        </template>
      </template>

      <!-- Wizard steps -->
      <template v-else>
        <Button v-if="step > 0" variant="ghost" @click="step = (step - 1) as Step">
          <ChevronLeft class="size-4" /> Back
        </Button>
        <Button v-else variant="ghost" @click="modalClose">Cancel</Button>

        <Button v-if="step === 0" :disabled="!basicsValid" @click="step = 1">Next</Button>
        <Button v-else-if="step === 1" :disabled="!stackValid" @click="step = 2">Next</Button>
        <Button v-else-if="step === 2" @click="step = 3">Next</Button>
        <Button v-else :disabled="!ready" @click="startCreate">Create site</Button>
      </template>
    </template>
  </Modal>
</template>

<style scoped>
/* Chevron/arrow breadcrumb: a right-pointing point with a matching concave
   left notch so the segments read as forward-pointing arrows. The first
   segment is flat on its left edge. */
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
/* The final segment has no next step, so it's a flat-right square (still notched
   on the left to receive the previous arrow). */
.step-chevron:last-child {
  clip-path: polygon(0 0, 100% 0, 100% 100%, 0 100%, 10px 50%);
}
/* 1px separator on the arrow junction. clip-path eats normal borders, so a
   drop-shadow offset *left* traces the segment's notch edge on top of the
   previous segment's arrow (a right offset would be hidden under the overlap). */
.step-chevron-sep {
  filter: drop-shadow(-2.5px 0 0 #a1a1aa);
}
</style>
