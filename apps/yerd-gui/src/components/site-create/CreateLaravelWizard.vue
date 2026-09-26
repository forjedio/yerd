<script setup lang="ts">
import { computed, reactive, ref, watch } from "vue";

import CreateSiteWizardShell from "@/components/site-create/CreateSiteWizardShell.vue";
import SiteBasicsStep from "@/components/site-create/SiteBasicsStep.vue";
import Input from "@/components/ui/Input.vue";
import Select from "@/components/ui/Select.vue";
import Switch from "@/components/ui/Switch.vue";
import { useSiteBasics } from "@/composables/useSiteBasics";
import { type PrereqRow, useToolPrereqs } from "@/composables/useToolPrereqs";
import { useToast } from "@/composables/useToast";
import { phpVersionInRange } from "@/lib/phpVersion";
import type {
  AuthProvider,
  CreateSiteSpec,
  Database,
  JsRuntime,
  LaravelOptions,
  StarterKit,
  StarterKitTag,
  StatusReport,
  Testing,
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

const PHASES = ["Preflight", "Scaffolding", "Registering", "Done"];

const form = reactive({
  name: "",
  location: "",
  php: "",
  secure: false,
  communityPackage: "",
  auth: "laravel" as AuthProvider,
  livewireClassComponents: false,
  teams: false,
  js: "npm" as JsRuntime,
  testing: "pest" as Testing,
  database: "sqlite" as Database,
  git: true,
  boost: false,
});
const { nameValid, projectPath, domain, openUrl, basicsValid } = useSiteBasics(form, props);

const KIT_OPTIONS: { value: StarterKitTag | "community"; label: string; hint: string }[] = [
  { value: "none", label: "None", hint: "Plain skeleton" },
  { value: "react", label: "React", hint: "Inertia + TS" },
  { value: "vue", label: "Vue", hint: "Inertia + TS" },
  { value: "livewire", label: "Livewire", hint: "Blade + PHP" },
  { value: "svelte", label: "Svelte", hint: "Inertia + TS" },
  { value: "community", label: "Community…", hint: "--using <package>" },
];

const kitChoice = ref<StarterKitTag | "community">("none");
const isJsKit = computed(
  () => ["react", "vue", "svelte"].includes(kitChoice.value) || kitChoice.value === "community",
);
const isLivewire = computed(() => kitChoice.value === "livewire");
const hasAuthKit = computed(() => kitChoice.value !== "none");
const stackValid = computed(
  () => kitChoice.value !== "community" || form.communityPackage.trim() !== "",
);

/** The Laravel installer only supports this window; anything outside it is hidden
 *  from the picker rather than offered and then failing mid-create. */
const LARAVEL_MIN_PHP = "8.3";
const LARAVEL_MAX_PHP = "8.5";
function supportsPhp(v: string): boolean {
  return phpVersionInRange(v, LARAVEL_MIN_PHP, LARAVEL_MAX_PHP);
}
const supportedPhpVersions = computed(() => props.phpVersions.filter(supportsPhp));
const phpOptions = computed(() =>
  supportedPhpVersions.value.map((v) => ({ value: v, label: `PHP ${v}` })),
);

/** The default only applies when it falls in the supported window; otherwise the
 *  newest supported version wins so the Basics step always opens on a valid pick. */
function preferredPhp(): string {
  const supported = supportedPhpVersions.value;
  if (props.defaultPhp && supported.includes(props.defaultPhp)) return props.defaultPhp;
  return supported[supported.length - 1] ?? "";
}

const prereqs = reactive(useToolPrereqs(() => props.open));
/** Building the managed Laravel installer needs Yerd's own Composer; an external one can't. */
const managedComposer = computed(() => prereqs.managed("composer"));
const needsComposer = computed(() => !prereqs.available("composer"));
const needsInstaller = computed(() => !prereqs.available("laravel"));
const noSupportedPhp = computed(() => supportedPhpVersions.value.length === 0);
/** Node and Bun are installed by the daemon during the job when needed, so they never block the wizard. */
const needsNode = computed(() => form.js === "npm" && !prereqs.available("node"));
const needsBun = computed(() => form.js === "bun" && !prereqs.available("bun"));
const ready = computed(
  () => !noSupportedPhp.value && !needsComposer.value && !needsInstaller.value,
);

function installPhp(): Promise<boolean> {
  return prereqs.installPhp(
    supportsPhp,
    `No installable PHP between ${LARAVEL_MIN_PHP} and ${LARAVEL_MAX_PHP} was found.`,
  );
}

const prereqRows = computed<PrereqRow[]>(() => [
  {
    id: "php",
    label: "PHP",
    sub:
      noSupportedPhp.value && props.phpVersions.length > 0
        ? `Installed, but not ${LARAVEL_MIN_PHP}-${LARAVEL_MAX_PHP}`
        : `Runtime ${LARAVEL_MIN_PHP}-${LARAVEL_MAX_PHP}`,
    ok: !noSupportedPhp.value,
    install: installPhp,
  },
  {
    id: "composer",
    label: "Composer",
    sub: "Dependency manager",
    ok: !needsComposer.value,
    install: () => prereqs.installTool("composer"),
  },
  {
    id: "laravel",
    label: "Laravel installer",
    sub: "laravel new",
    ok: !needsInstaller.value,
    install: () => prereqs.installTool("laravel"),
    blockedReason: managedComposer.value
      ? undefined
      : "Yerd's own Composer is required to build the Laravel installer",
  },
]);

/** PHP, then Composer, then the installer, which only Yerd's own Composer can build. */
function installAll(): Promise<void> {
  return prereqs.installAll(async () => {
    if (noSupportedPhp.value && !(await installPhp())) return false;
    if (needsComposer.value && !(await prereqs.installTool("composer"))) return false;
    if (needsInstaller.value) {
      if (!managedComposer.value) {
        toast.error(
          "Can't install the Laravel installer",
          "Yerd needs its own Composer to build it - install Yerd's Composer, or run `composer global require laravel/installer`.",
        );
        return false;
      }
      if (!(await prereqs.installTool("laravel"))) return false;
    }
    return true;
  });
}

/** The JS runtime picker only shows for Inertia kits, so None and Livewire never
 *  trigger an npm or Node install the user didn't ask for. */
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

function reset(): void {
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
  prereqs.reset();
  void prereqs.refresh();
}

/** Keep the selection honest as the installed set changes underneath the wizard:
 *  a version no longer supported falls back to the preferred one. */
watch(
  () => props.phpVersions,
  () => {
    if (!form.php || !supportedPhpVersions.value.includes(form.php)) {
      form.php = preferredPhp();
    }
  },
);
</script>

<template>
  <CreateSiteWizardShell
    :open="open"
    title="Create a new Laravel site"
    :steps="['Basics', 'Stack', 'Testing', 'Review']"
    :step-valid="[basicsValid, stackValid, true, true]"
    :build-spec="buildSpec"
    :phases="PHASES"
    :domain="domain"
    :project-path="projectPath"
    :open-url="openUrl"
    :prereqs="prereqs"
    :prereq-rows="prereqRows"
    prereq-summary="Creating a Laravel site needs PHP, Composer and the Laravel installer."
    :ready="ready"
    :install-all="installAll"
    @update:open="(v) => emit('update:open', v)"
    @created="emit('created')"
    @reset="reset"
  >
    <template #step-0>
      <SiteBasicsStep
        v-model:name="form.name"
        v-model:location="form.location"
        v-model:php="form.php"
        v-model:secure="form.secure"
        id-prefix="cs"
        :parked-folders="parkedFolders"
        :php-options="phpOptions"
        :domain="domain"
        :project-path="projectPath"
        :name-valid="nameValid"
        no-php-text="No supported PHP installed."
      >
        <template #php-hint>
          The version this site runs on. Laravel needs PHP {{ LARAVEL_MIN_PHP }} to
          {{ LARAVEL_MAX_PHP }}.
        </template>
      </SiteBasicsStep>
    </template>

    <template #step-1>
      <div class="space-y-4">
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
            <p v-if="needsNode || needsBun" class="mt-1 text-xs text-muted-foreground">
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
    </template>

    <template #step-2>
      <div class="space-y-4">
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
              <p class="text-xs text-muted-foreground">
                Run <code class="font-mono">git init</code> in the new project.
              </p>
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
    </template>

    <template #step-3>
      <div class="space-y-4">
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
    </template>
  </CreateSiteWizardShell>
</template>
