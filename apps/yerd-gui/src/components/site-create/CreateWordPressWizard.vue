<script setup lang="ts">
import { computed, reactive, ref, watch } from "vue";
import { ExternalLink, RefreshCw } from "lucide-vue-next";

import CreateSiteWizardShell from "@/components/site-create/CreateSiteWizardShell.vue";
import SiteBasicsStep from "@/components/site-create/SiteBasicsStep.vue";
import Button from "@/components/ui/Button.vue";
import Combobox from "@/components/ui/Combobox.vue";
import Input from "@/components/ui/Input.vue";
import Select from "@/components/ui/Select.vue";
import { useSiteBasics } from "@/composables/useSiteBasics";
import { type PrereqRow, useToolPrereqs } from "@/composables/useToolPrereqs";
import { isLegacyVersion, phpVersionInRange } from "@/lib/phpVersion";
import { isUnbound, wpAdminLoginUrl, wpAdminUrl } from "@/lib/siteUrl";
import { WORDPRESS_LOCALES } from "@/lib/wordpressLocales";
import {
  availableWordPressVersions,
  listServices,
  mintWordPressLoginToken,
  openInBrowser,
} from "@/ipc/client";
import type {
  CreateSiteSpec,
  ServiceStatus,
  StatusReport,
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

/** Mirrors the `set_phase` strings `bin/yerdd/src/create_site/wordpress.rs::run` emits, in order. */
const PHASES = [
  "Preflight",
  "Provisioning database",
  "Downloading WordPress",
  "Configuring",
  "Installing",
  "Registering",
  "Done",
];

const form = reactive({
  name: "",
  location: "",
  php: "",
  secure: false,
  coreVersion: "",
  locale: "en_US",
  adminUser: "admin",
  adminEmail: "test@example.com",
  adminPassword: randomPassword(),
  siteTitle: "",
  tablePrefix: "wp_",
  dbEngine: "mysql" as WordPressDbEngine,
  dbName: "",
});
let dbNameTouched = false;
const { siteName, nameValid, projectPath, domain, openUrl, basicsValid } = useSiteBasics(
  form,
  props,
);

/** A native `<select>` can't host a Badge, so legacy is called out in the option
 *  label itself and expanded on by the hint below the picker. */
const phpOptions = computed(() =>
  props.phpVersions.map((v) => ({
    value: v,
    label: isLegacyVersion(v) ? `PHP ${v} (legacy)` : `PHP ${v}`,
  })),
);
const phpIsLegacy = computed(() => !!form.php && isLegacyVersion(form.php));

const adminUrl = computed(() =>
  wpAdminUrl({ name: siteName.value || "name", secure: form.secure }, props.report),
);

/**
 * "WP Admin" on the success screen: a one-click, pre-authenticated login when
 * possible, falling back to the plain (not signed-in) link when unbound or if
 * minting a token fails for any reason. It never blocks or surfaces an error.
 */
async function openWpAdmin(): Promise<void> {
  const site = { name: siteName.value, secure: form.secure };
  if (!isUnbound(props.report)) {
    const signedIn = await mintWordPressLoginToken(site.name)
      .then((token) => openInBrowser(wpAdminLoginUrl(site, props.report, token)))
      .then(
        () => true,
        () => false,
      );
    if (signedIn) return;
  }
  await openInBrowser(adminUrl.value);
}

const emailValid = computed(() => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(form.adminEmail.trim()));
const wordpressValid = computed(
  () =>
    form.adminUser.trim() !== "" &&
    emailValid.value &&
    form.adminPassword.length >= 8 &&
    form.siteTitle.trim() !== "",
);
const dbNameValid = computed(() => /^[A-Za-z_][A-Za-z0-9_]{0,62}$/.test(form.dbName));

/**
 * A valid database name derived from the site name, mirroring
 * `bin/yerdd/src/create_site/wordpress.rs::derive_db_name`. The daemon is the
 * authority and re-validates, but pre-filling with the same rule avoids a
 * rejected default: hyphens become underscores, a leading letter is added if
 * needed, and the result is capped at 63 characters.
 */
function deriveDbName(name: string): string {
  let db = name.replace(/-/g, "_");
  if (!/^[A-Za-z_]/.test(db)) db = `wp_${db}`;
  return db.slice(0, 63);
}

watch(
  () => form.name,
  (name) => {
    if (!dbNameTouched) form.dbName = deriveDbName(name.trim());
  },
);

function randomPassword(): string {
  const chars = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789!@#$%^&*";
  const bytes = new Uint32Array(20);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => chars[b % chars.length]).join("");
}

const prereqs = reactive(useToolPrereqs(() => props.open));
/** Building WP-CLI needs Yerd's own Composer, the same asymmetry as the Laravel
 *  installer. The daemon never reports WP-CLI as `external`: scaffolding execs
 *  Yerd's own boot-fs.php rather than a PATH-resolved `wp`. */
const managedComposer = computed(() => prereqs.managed("composer"));
const needsWpCli = computed(() => !prereqs.available("wp-cli"));
/** Composer only builds WP-CLI here, so it's needed exactly while WP-CLI is. */
const needsComposer = computed(() => needsWpCli.value && !managedComposer.value);
const noPhp = computed(() => props.phpVersions.length === 0);
const ready = computed(() => !noPhp.value && !needsComposer.value && !needsWpCli.value);

function installPhp(): Promise<boolean> {
  return prereqs.installPhp(() => true, "No installable PHP versions were found.");
}

/** Composer is listed only while WP-CLI still has to be built; once WP-CLI is
 *  present, "Installed" would claim something that wasn't checked. */
const prereqRows = computed<PrereqRow[]>(() => [
  { id: "php", label: "PHP", sub: "Runtime", ok: !noPhp.value, install: installPhp },
  ...(needsWpCli.value
    ? [
        {
          id: "composer",
          label: "Composer",
          sub: "Builds WP-CLI",
          ok: !needsComposer.value,
          install: () => prereqs.installTool("composer"),
        },
      ]
    : []),
  {
    id: "wp-cli",
    label: "WP-CLI",
    sub: "wp command",
    ok: !needsWpCli.value,
    install: () => prereqs.installTool("wp-cli"),
    blockedReason: managedComposer.value
      ? undefined
      : "Yerd's own Composer is required to build WP-CLI",
  },
]);

function installAll(): Promise<void> {
  return prereqs.installAll(async () => {
    if (noPhp.value && !(await installPhp())) return false;
    if (needsComposer.value && !(await prereqs.installTool("composer"))) return false;
    if (needsWpCli.value && !(await prereqs.installTool("wp-cli"))) return false;
    return true;
  });
}

const services = ref<ServiceStatus[]>([]);
const servicesLoading = ref(false);
let dbEngineTouched = false;

/** Prefer a running MySQL/MariaDB over a merely installed one; `null` when
 *  neither is installed, which leaves the default. Provisioning itself happens
 *  inside the create job. */
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

/** Core releases from the daemon-cached `meta/wordpress-versions.json` (see `wordpress_versions.rs`). */
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

/** Values are the concrete latest patch (`wp core download --version=` needs an
 *  exact release; a bare branch resolves to its unpatched original), labelled
 *  by the friendlier branch name. */
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

function reset(): void {
  form.name = "";
  form.location = props.parkedFolders[0] ?? "";
  form.php = props.defaultPhp || props.phpVersions[0] || "";
  form.secure = false;
  form.coreVersion = "";
  form.locale = "en_US";
  form.adminUser = "admin";
  form.adminEmail = "test@example.com";
  form.adminPassword = randomPassword();
  form.siteTitle = "";
  form.tablePrefix = "wp_";
  form.dbEngine = "mysql";
  form.dbName = "";
  dbNameTouched = false;
  dbEngineTouched = false;
  prereqs.reset();
  void prereqs.refresh();
  void refreshServices();
  void refreshWordpressVersions();
}

watch(
  () => props.phpVersions,
  (versions) => {
    if (!form.php && versions.length) {
      form.php = props.defaultPhp || versions[0];
    }
  },
);
</script>

<template>
  <CreateSiteWizardShell
    :open="open"
    title="Create a new WordPress site"
    :steps="['Basics', 'WordPress', 'Database', 'Review']"
    :step-valid="[basicsValid, wordpressValid, dbNameValid, true]"
    :build-spec="buildSpec"
    :phases="PHASES"
    :domain="domain"
    :project-path="projectPath"
    :open-url="openUrl"
    :prereqs="prereqs"
    :prereq-rows="prereqRows"
    prereq-summary="Creating a WordPress site needs PHP and WP-CLI (which Yerd's own Composer builds)."
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
        id-prefix="wp-cs"
        :parked-folders="parkedFolders"
        :php-options="phpOptions"
        :domain="domain"
        :project-path="projectPath"
        :name-valid="nameValid"
        no-php-text="No PHP installed."
      >
        <template #php-hint>
          <template v-if="phpIsLegacy">
            Out of support: no dumps or coverage, and it can't be your default.
          </template>
          <template v-else>The version this site runs on.</template>
        </template>
      </SiteBasicsStep>
    </template>

    <template #step-1>
      <div class="space-y-4">
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
            <Button variant="outline" @click="form.adminPassword = randomPassword()">
              <RefreshCw class="size-4" /> Generate
            </Button>
          </div>
        </div>
      </div>
    </template>

    <template #step-2>
      <div class="space-y-4">
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

        <div
          v-if="!servicesLoading && engineStateText"
          class="rounded-lg border bg-muted/30 p-3 text-xs text-muted-foreground"
        >
          {{ engineStateText }}
        </div>
        <p class="text-xs text-muted-foreground">
          Only MySQL and MariaDB are supported for WordPress core. Yerd provisions the database as
          part of creating this site.
        </p>
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
    </template>

    <template #success-actions>
      <Button
        variant="outline"
        title="Signs you in as the site's admin when possible"
        @click="openWpAdmin"
      >
        <ExternalLink class="size-4" /> WP Admin
      </Button>
    </template>
  </CreateSiteWizardShell>
</template>
