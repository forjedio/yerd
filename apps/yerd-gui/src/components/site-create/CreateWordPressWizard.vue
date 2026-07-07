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
  RefreshCw,
  TriangleAlert,
} from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import Combobox from "@/components/ui/Combobox.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Select from "@/components/ui/Select.vue";
import Switch from "@/components/ui/Switch.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { phpVersionInRange } from "@/lib/phpVersion";
import { isUnbound, siteUrl, wpAdminLoginUrl, wpAdminUrl } from "@/lib/siteUrl";
import { WORDPRESS_LOCALES } from "@/lib/wordpressLocales";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  availablePhp,
  availableWordPressVersions,
  createSite,
  installPhpWithProgress,
  installToolStreamed,
  IpcError,
  jobCancel,
  jobStatus,
  listServices,
  listTools,
  mintWordPressLoginToken,
  openInBrowser,
  openPath,
  pickDirectory,
  pollJobToEnd,
} from "@/ipc/client";
import type {
  CreateSiteSpec,
  JobState,
  ServiceStatus,
  StatusReport,
  ToolStatus,
  WordPressDbEngine,
  WordPressOptions,
  WordPressVersionInfo,
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
type Step = 0 | 1 | 2 | 3 | 4; // Basics, WordPress, Database, Review, Progress
const step = ref<Step>(0);

const STEP_LABELS = ["Basics", "WordPress", "Database", "Review"];

const form = reactive({
  name: "",
  location: "",
  php: "",
  secure: false,
  // WordPress
  coreVersion: "",
  locale: "en_US",
  adminUser: "admin",
  adminEmail: "test@example.com",
  adminPassword: "password",
  siteTitle: "",
  tablePrefix: "wp_",
  // database
  dbEngine: "mysql" as WordPressDbEngine,
  dbName: "",
});
let dbNameTouched = false;

const phpOptions = computed(() =>
  props.phpVersions.map((v) => ({ value: v, label: `PHP ${v}` })),
);
const locationOptions = computed(() => {
  const opts = props.parkedFolders.map((f) => ({ value: f, label: `${f}  (parked)` }));
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
const openUrl = computed(() =>
  siteUrl({ name: form.name.trim().toLowerCase() || "name", secure: form.secure }, props.report),
);
const adminUrl = computed(() =>
  wpAdminUrl({ name: form.name.trim().toLowerCase() || "name", secure: form.secure }, props.report),
);

/**
 * "WP Admin" action on the post-creation success screen: one-click,
 * pre-authenticated login when possible, falling back to the plain
 * (not signed-in) link when unbound/resolver-off, or if minting a token
 * fails for any reason - never blocks or surfaces an error.
 */
async function openWpAdmin(): Promise<void> {
  const site = { name: form.name.trim().toLowerCase(), secure: form.secure };
  if (!isUnbound(props.report)) {
    try {
      const token = await mintWordPressLoginToken(site.name);
      await openInBrowser(wpAdminLoginUrl(site, props.report, token));
      return;
    } catch {
      /* fall through to the plain link below */
    }
  }
  await openInBrowser(adminUrl.value);
}

const basicsValid = computed(
  () => nameValid.value && form.location.trim() !== "" && form.php !== "",
);
const emailValid = computed(() => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(form.adminEmail.trim()));
const wordpressValid = computed(
  () =>
    form.adminUser.trim() !== "" &&
    emailValid.value &&
    form.adminPassword.length >= 8 &&
    form.siteTitle.trim() !== "",
);
const dbNameValid = computed(() => /^[A-Za-z_][A-Za-z0-9_]{0,62}$/.test(form.dbName));

// Derive a valid database name from the site name (mirrors
// `bin/yerdd/src/create_site/wordpress.rs::derive_db_name` - the daemon is the
// authority and re-validates whatever is submitted, but pre-filling with the
// same rule avoids surprising the user with a rejected default): map hyphens
// to underscores, prefix with a letter if the result still doesn't start with
// one, then cap at 63 chars.
function deriveDbName(siteName: string): string {
  let name = siteName.replace(/-/g, "_");
  if (!/^[A-Za-z_]/.test(name)) name = `wp_${name}`;
  return name.slice(0, 63);
}

watch(
  () => form.name,
  (name) => {
    if (!dbNameTouched) form.dbName = deriveDbName(name.trim());
  },
);

function generatePassword(): void {
  const chars = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789!@#$%^&*";
  const bytes = new Uint32Array(20);
  crypto.getRandomValues(bytes);
  form.adminPassword = Array.from(bytes, (b) => chars[b % chars.length]).join("");
}

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

function toolAvailable(id: string): boolean {
  return tools.value.some((t) => t.id === id && (t.installed || t.external));
}
// Managed-only: building WP-CLI requires Yerd's own Composer (an external
// Composer can't build it) - same asymmetry as the Laravel installer.
const managedComposer = computed(() =>
  tools.value.some((t) => t.id === "composer" && t.installed),
);
const needsComposer = computed(() => !toolAvailable("composer"));
const needsWpCli = computed(() => !toolAvailable("wp-cli"));
const noPhp = computed(() => props.phpVersions.length === 0);

const ready = computed(() => !noPhp.value && !needsComposer.value && !needsWpCli.value);
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

async function installPrereq(id: "composer" | "wp-cli"): Promise<boolean> {
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

async function installFirstPhp(): Promise<boolean> {
  installingTool.value = "php";
  installLog.value = [];
  try {
    const { available } = await availablePhp();
    const version = available[available.length - 1];
    if (!version) {
      toast.error("Couldn't install PHP", "No installable PHP versions were found.");
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
    if (noPhp.value && !(await installFirstPhp())) return;
    if (needsComposer.value && !(await installPrereq("composer"))) return;
    if (needsWpCli.value) {
      if (!managedComposer.value) {
        toast.error(
          "Can't install WP-CLI",
          "Yerd needs its own Composer to build it - install Yerd's Composer first.",
        );
        return;
      }
      if (!(await installPrereq("wp-cli"))) return;
    }
    await nextTick();
    toast.success("Toolchain ready");
  } finally {
    installingAll.value = false;
  }
}

// ── database engine detection (informational only - provisioning happens
// inline in the create job, see the Progress step's "Provisioning database"
// phase) ─────────────────────────────────────────────────────────────────────
const services = ref<ServiceStatus[]>([]);
const servicesLoading = ref(false);
let dbEngineTouched = false;

/** Prefer a running engine over a merely-installed one (WP-P1-07: "Reuse a
 *  running SQL engine" - preselect whichever engine is actually available),
 *  `null` when neither MySQL nor MariaDB is installed (leaves the default). */
function preferredEngine(statuses: ServiceStatus[]): WordPressDbEngine | null {
  const candidates = statuses.filter(
    (s) => (s.service === "mysql" || s.service === "mariadb") && s.installed_versions.length > 0,
  );
  const running = candidates.find((s) => s.state === "running");
  if (running) return running.service as WordPressDbEngine;
  return candidates.length ? (candidates[0].service as WordPressDbEngine) : null;
}

async function refreshServices(): Promise<void> {
  servicesLoading.value = true;
  try {
    services.value = await listServices();
    if (!dbEngineTouched) {
      const preferred = preferredEngine(services.value);
      if (preferred) form.dbEngine = preferred;
    }
  } catch {
    services.value = [];
  } finally {
    servicesLoading.value = false;
  }
}

function selectDbEngine(engine: WordPressDbEngine): void {
  dbEngineTouched = true;
  form.dbEngine = engine;
}

// ── WordPress core version (from meta/wordpress-versions.json in the yerd
// repo, daemon-fetched and cached - see bin/yerdd/src/wordpress_versions.rs)
const wordpressVersions = ref<WordPressVersionInfo[]>([]);
const wordpressVersionsLoading = ref(false);

async function refreshWordpressVersions(): Promise<void> {
  wordpressVersionsLoading.value = true;
  try {
    wordpressVersions.value = await availableWordPressVersions();
  } catch {
    wordpressVersions.value = [];
  } finally {
    wordpressVersionsLoading.value = false;
  }
}

const compatibleVersions = computed(() =>
  form.php
    ? wordpressVersions.value.filter((v) => phpVersionInRange(form.php, v.min_php, v.max_php))
    : wordpressVersions.value,
);

// The Select's value is the concrete latest patch (`wp core download
// --version=` needs an exact release - a bare branch like "6.7" resolves to
// that branch's original, unpatched release), labelled by its friendlier
// branch name.
const versionOptions = computed(() => [
  { value: "", label: "Latest" },
  ...compatibleVersions.value.map((v) => ({ value: v.latest, label: v.branch })),
]);

watch(
  () => form.php,
  () => {
    if (form.coreVersion && !compatibleVersions.value.some((v) => v.latest === form.coreVersion)) {
      form.coreVersion = "";
    }
  },
);

const selectedEngineStatus = computed(() =>
  services.value.find((s) => s.service === form.dbEngine),
);
const engineStateText = computed(() => {
  const s = selectedEngineStatus.value;
  if (!s) return "";
  if (s.installed_versions.length === 0) {
    return `No ${s.display_name} found - Yerd will install and start it as part of creating this site.`;
  }
  if (s.state !== "running") {
    return `${s.display_name} is installed but not running - Yerd will start it as part of creating this site.`;
  }
  return `${s.display_name} is running and ready.`;
});

function buildSpec(): CreateSiteSpec {
  const options: WordPressOptions = {
    core_version: form.coreVersion.trim() || null,
    locale: form.locale.trim() || "en_US",
    admin_user: form.adminUser.trim(),
    admin_email: form.adminEmail.trim(),
    admin_password: form.adminPassword,
    site_title: form.siteTitle.trim(),
    table_prefix: form.tablePrefix.trim() || "wp_",
    database: {
      engine: form.dbEngine,
      name: form.dbName.trim(),
    },
  };
  return {
    name: form.name.trim(),
    parent_dir: form.location,
    php: form.php,
    secure: form.secure,
    framework: { framework: "wordpress", options },
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

const PHASES = ["Preflight", "Installing", "Registering", "Done"];
function phaseStatus(p: string): "done" | "active" | "todo" {
  const cur = phase.value;
  const ci = PHASES.indexOf(cur);
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
      cursor = r.next_cursor;
      phase.value = r.phase;
      jobStateRef.value = r.state;
      jobError.value = r.error;
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
  form.php = props.defaultPhp || props.phpVersions[0] || "";
  form.secure = false;
  form.coreVersion = "";
  form.locale = "en_US";
  form.adminUser = "admin";
  form.adminEmail = "test@example.com";
  form.adminPassword = "password";
  form.siteTitle = "";
  form.tablePrefix = "wp_";
  form.dbEngine = "mysql";
  form.dbName = "";
  dbNameTouched = false;
  dbEngineTouched = false;
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
      void refreshServices();
      void refreshWordpressVersions();
    } else {
      stopPolling();
    }
  },
);

watch(
  () => props.phpVersions,
  (versions) => {
    if (!form.php && versions.length) {
      form.php = props.defaultPhp || versions[0];
    }
  },
);

onUnmounted(stopPolling);

const busy = computed(() => jobStateRef.value === "running" && step.value === 4);
</script>

<template>
  <Modal
    :open="open"
    title="Create a new WordPress site"
    size="lg"
    @update:open="(v) => emit('update:open', v)"
  >
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
            Creating a WordPress site needs PHP, Composer and WP-CLI. Install the missing ones to
            continue.
          </p>
        </div>
      </div>

      <div class="divide-y rounded-lg border">
        <div
          v-for="row in [
            { id: 'php', label: 'PHP', sub: 'Runtime', ok: !noPhp },
            { id: 'composer', label: 'Composer', sub: 'Dependency manager', ok: !needsComposer },
            { id: 'wp-cli', label: 'WP-CLI', sub: 'wp command', ok: !needsWpCli },
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
                :disabled="installBusy || (row.id === 'wp-cli' && !managedComposer)"
                :title="row.id === 'wp-cli' && !managedComposer ? 'Yerd\'s own Composer is required to build WP-CLI' : ''"
                @click="row.id === 'php' ? installFirstPhp() : installPrereq(row.id as 'composer' | 'wp-cli')"
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
        <label class="text-sm font-medium" for="wp-cs-name">Project name</label>
        <Input id="wp-cs-name" v-model="form.name" placeholder="e.g. blog" class="mt-2" />
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
        <label class="text-sm font-medium" for="wp-cs-location">Location</label>
        <div class="mt-2 flex gap-2">
          <Select
            v-if="locationOptions.length"
            id="wp-cs-location"
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
          <p class="text-xs text-muted-foreground">The version this site runs on.</p>
        </div>
        <Select
          v-if="phpOptions.length"
          id="wp-cs-php"
          :model-value="form.php"
          :options="phpOptions"
          class="w-40 shrink-0"
          aria-label="PHP version"
          @update:model-value="(v: string) => (form.php = v)"
        />
        <span v-else class="shrink-0 text-xs text-destructive">No PHP installed.</span>
      </div>

      <div class="flex items-center justify-between gap-4 rounded-lg border p-3">
        <div>
          <p class="text-sm font-medium">HTTPS</p>
          <p class="text-xs text-muted-foreground">Serve this site over TLS.</p>
        </div>
        <Switch v-model="form.secure" aria-label="Serve over HTTPS" />
      </div>
    </div>

    <!-- ── Step 2: WordPress ── -->
    <div v-else-if="step === 1" class="space-y-4">
      <div class="flex gap-4">
        <div class="flex-1">
          <label class="text-sm font-medium" for="wp-cs-version">Core version</label>
          <Select
            id="wp-cs-version"
            :model-value="form.coreVersion"
            :options="versionOptions"
            :disabled="wordpressVersionsLoading"
            class="mt-2 w-full"
            aria-label="Core version"
            @update:model-value="(v: string) => (form.coreVersion = v)"
          />
        </div>
        <div class="flex-1">
          <label class="text-sm font-medium" for="wp-cs-locale">Locale</label>
          <Combobox
            v-model="form.locale"
            :options="WORDPRESS_LOCALES"
            placeholder="en_US"
            search-placeholder="Search locales…"
            empty-text="No matching locale."
            aria-label="Locale"
            class="mt-2"
          />
        </div>
      </div>

      <div>
        <label class="text-sm font-medium" for="wp-cs-title">Site title</label>
        <Input id="wp-cs-title" v-model="form.siteTitle" placeholder="My Blog" class="mt-2" />
      </div>

      <div class="flex gap-4">
        <div class="flex-1">
          <label class="text-sm font-medium" for="wp-cs-admin-user">Admin username</label>
          <Input id="wp-cs-admin-user" v-model="form.adminUser" class="mt-2" />
        </div>
        <div class="flex-1">
          <label class="text-sm font-medium" for="wp-cs-admin-email">Admin email</label>
          <Input id="wp-cs-admin-email" v-model="form.adminEmail" type="email" class="mt-2" />
          <p v-if="form.adminEmail && !emailValid" class="mt-1 text-xs text-destructive">
            Enter a valid email address.
          </p>
        </div>
      </div>

      <div>
        <label class="text-sm font-medium" for="wp-cs-admin-password">Admin password</label>
        <div class="mt-2 flex gap-2">
          <Input
            id="wp-cs-admin-password"
            v-model="form.adminPassword"
            type="text"
            placeholder="At least 8 characters"
            class="font-mono"
          />
          <Button variant="outline" @click="generatePassword">
            <RefreshCw class="size-4" /> Generate
          </Button>
        </div>
      </div>
    </div>

    <!-- ── Step 3: Database ── -->
    <div v-else-if="step === 2" class="space-y-4">
      <div>
        <span class="text-sm font-medium">Database engine</span>
        <div class="mt-2 grid grid-cols-2 gap-2">
          <button
            v-for="opt in [
              { value: 'mysql', label: 'MySQL' },
              { value: 'mariadb', label: 'MariaDB' },
            ]"
            :key="opt.value"
            type="button"
            class="rounded-lg border p-2.5 text-left transition-colors"
            :class="
              form.dbEngine === opt.value
                ? 'border-brand bg-brand/5 ring-1 ring-brand'
                : 'hover:border-brand/40'
            "
            @click="selectDbEngine(opt.value as WordPressDbEngine)"
          >
            <span class="block text-sm font-medium">{{ opt.label }}</span>
          </button>
        </div>
      </div>

      <div class="flex gap-4">
        <div class="flex-1">
          <label class="text-sm font-medium" for="wp-cs-dbname">Database name</label>
          <Input
            id="wp-cs-dbname"
            v-model="form.dbName"
            class="mt-2 font-mono"
            @update:model-value="dbNameTouched = true"
          />
          <p v-if="form.dbName && !dbNameValid" class="mt-1 text-xs text-destructive">
            Use letters, numbers and underscores, starting with a letter or underscore.
          </p>
        </div>
        <div class="w-32 shrink-0">
          <label class="text-sm font-medium" for="wp-cs-table-prefix">Table prefix</label>
          <Input id="wp-cs-table-prefix" v-model="form.tablePrefix" class="mt-2 font-mono" />
        </div>
      </div>

      <div v-if="!servicesLoading && engineStateText" class="rounded-lg border bg-muted/30 p-3 text-xs text-muted-foreground">
        {{ engineStateText }}
      </div>
      <p class="text-xs text-muted-foreground">
        Only MySQL and MariaDB are supported for WordPress core. Yerd provisions the database as
        part of creating this site.
      </p>
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
          <dt class="text-muted-foreground">WordPress</dt>
          <dd>{{ form.coreVersion || "Latest" }} · {{ form.locale }}</dd>
          <dt class="text-muted-foreground">Admin</dt>
          <dd>{{ form.adminUser }} ({{ form.adminEmail }})</dd>
          <dt class="text-muted-foreground">Database</dt>
          <dd>{{ form.dbEngine }} · {{ form.dbName }}</dd>
        </dl>
      </div>
      <p v-if="!wordpressValid" class="text-xs text-destructive">
        Fill in the admin username, a valid email and a password of at least 8 characters on the
        WordPress step before continuing.
      </p>
      <p v-else-if="!dbNameValid" class="text-xs text-destructive">
        Fix the database name on the Database step before continuing.
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
          <Button
            variant="outline"
            title="Signs you in as the site's admin when possible"
            @click="openWpAdmin"
          >
            <ExternalLink class="size-4" /> WP Admin
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

      <template v-else>
        <Button v-if="step > 0" variant="ghost" @click="step = (step - 1) as Step">
          <ChevronLeft class="size-4" /> Back
        </Button>
        <Button v-else variant="ghost" @click="modalClose">Cancel</Button>

        <Button v-if="step === 0" :disabled="!basicsValid" @click="step = 1">Next</Button>
        <Button v-else-if="step === 1" :disabled="!wordpressValid" @click="step = 2">Next</Button>
        <Button v-else-if="step === 2" :disabled="!dbNameValid" @click="step = 3">Next</Button>
        <Button v-else :disabled="!ready" @click="startCreate">Create site</Button>
      </template>
    </template>
  </Modal>
</template>

<style scoped>
/* Chevron/arrow breadcrumb - identical to CreateLaravelWizard.vue's. */
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
