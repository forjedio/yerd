import { computed, reactive, ref, watch } from "vue";

import type { SiteBasicsForm } from "@/composables/useSiteBasics";
import { preferredPhp, type StarterContext, type StarterModel } from "@/composables/useSiteStarters";
import type { PrereqRow } from "@/composables/useToolPrereqs";
import {
  availableWordPressVersions,
  listServices,
  mintWordPressLoginToken,
  openInBrowser,
} from "@/ipc/client";
import type {
  CreateSiteSpec,
  ServiceStatus,
  WordPressDbEngine,
  WordPressOptions,
  WordPressVersionInfo,
} from "@/ipc/types";
import { isLegacyVersion, phpVersionInRange } from "@/lib/phpVersion";
import { isUnbound, wpAdminLoginUrl, wpAdminUrl } from "@/lib/siteUrl";

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

const ENGINE_LABELS: Record<WordPressDbEngine, string> = { mysql: "MySQL", mariadb: "MariaDB" };

function randomPassword(): string {
  const chars = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789!@#$%^&*";
  const bytes = new Uint32Array(20);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => chars[b % chars.length]).join("");
}

/**
 * A valid database name derived from the site name, mirroring
 * `bin/yerdd/src/create_site/wordpress.rs::derive_db_name`. The daemon is the
 * authority and re-validates, but pre-filling with the same rule avoids a
 * rejected default: hyphens become underscores, a leading letter is added if
 * needed, and the result is capped at 63 characters.
 */
export function deriveDbName(name: string): string {
  let db = name.replace(/-/g, "_");
  if (!/^[A-Za-z_]/.test(db)) db = `wp_${db}`;
  return db.slice(0, 63);
}

/**
 * The WordPress starter: core version and locale, site title, the admin account,
 * the database engine and name, plus the toolchain gate (PHP and WP-CLI, which
 * Yerd's own Composer builds) and the Done screen's WP Admin link.
 */
export function useWordPressStarter(basics: SiteBasicsForm, ctx: StarterContext) {
  const prereqs = ctx.prereqs;

  const form = reactive({
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

  const siteName = computed(() => basics.name.trim().toLowerCase());

  const adminUrl = computed(() =>
    wpAdminUrl({ name: siteName.value || "name", secure: basics.secure }, ctx.report()),
  );

  /**
   * "WP Admin" on the Done screen: a one-click, pre-authenticated login when
   * possible, falling back to the plain (not signed-in) link when unbound or if
   * minting a token fails for any reason. It never blocks or surfaces an error.
   */
  async function openWpAdmin(): Promise<void> {
    const site = { name: siteName.value, secure: basics.secure };
    const report = ctx.report();
    if (!isUnbound(report)) {
      const signedIn = await mintWordPressLoginToken(site.name)
        .then((token) => openInBrowser(wpAdminLoginUrl(site, report, token)))
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

  watch(
    () => basics.name,
    (name) => {
      if (!dbNameTouched) form.dbName = deriveDbName(name.trim());
    },
  );

  /** The database name field was edited by hand: stop deriving it from the site name. */
  function touchDbName(): void {
    dbNameTouched = true;
  }

  function regeneratePassword(): void {
    form.adminPassword = randomPassword();
  }

  /** Building WP-CLI needs Yerd's own Composer, the same asymmetry as the Laravel
   *  installer. The daemon never reports WP-CLI as `external`: scaffolding execs
   *  Yerd's own boot-fs.php rather than a PATH-resolved `wp`. */
  const managedComposer = computed(() => prereqs.managed("composer"));
  const needsWpCli = computed(() => !prereqs.available("wp-cli"));
  /** Composer only builds WP-CLI here, so it's needed exactly while WP-CLI is. */
  const needsComposer = computed(() => needsWpCli.value && !managedComposer.value);
  const noPhp = computed(() => ctx.phpVersions().length === 0);
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
    basics.php
      ? wordpressVersions.value.filter((v) => phpVersionInRange(basics.php, v.min_php, v.max_php))
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
    () => basics.php,
    () => {
      if (
        form.coreVersion &&
        !compatibleVersions.value.some((v) => v.latest === form.coreVersion)
      ) {
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
      name: basics.name.trim(),
      parent_dir: basics.location,
      php: basics.php,
      secure: basics.secure,
      framework: { framework: "wordpress", options },
    };
  }

  const doneSummary = computed(() =>
    [
      `WordPress ${form.coreVersion || "latest"}`,
      form.locale,
      ENGINE_LABELS[form.dbEngine],
      `PHP ${basics.php}`,
    ].join(" · "),
  );

  /** Restores the options and refetches what they list. The database name is
   *  derived from whatever name the Project step already holds, so a name typed
   *  before switching to WordPress (or prefilled) still gets its default. */
  function reset(): void {
    form.coreVersion = "";
    form.locale = "en_US";
    form.adminUser = "admin";
    form.adminEmail = "test@example.com";
    form.adminPassword = randomPassword();
    form.siteTitle = "";
    form.tablePrefix = "wp_";
    form.dbEngine = "mysql";
    dbNameTouched = false;
    form.dbName = deriveDbName(basics.name.trim());
    dbEngineTouched = false;
    void refreshServices();
    void refreshWordpressVersions();
  }

  const model: StarterModel = {
    id: "wordpress",
    label: "WordPress",
    phases: PHASES,
    supportsPhp: () => true,
    preferredPhp: () => preferredPhp(ctx.phpVersions(), ctx.defaultPhp(), () => true),
    phpLabel: (v) => (isLegacyVersion(v) ? `${v} (legacy)` : v),
    phpHint: (php) =>
      php && isLegacyVersion(php)
        ? "Out of support: no dumps or coverage, and it can't be your default."
        : "The version this site runs on.",
    noPhpText: "No PHP installed.",
    prereqRows,
    prereqSummary: "Creating a WordPress site needs PHP and WP-CLI (which Yerd's own Composer builds).",
    ready,
    installAll,
    optionsValid: computed(() => wordpressValid.value && dbNameValid.value),
    buildSpec,
    doneSummary,
    reset,
  };

  return {
    ...model,
    form,
    emailValid,
    wordpressValid,
    dbNameValid,
    touchDbName,
    regeneratePassword,
    selectDbEngine,
    servicesLoading,
    engineStateText,
    versionOptions,
    wordpressVersionsLoading,
    openWpAdmin,
  };
}

/** The WordPress starter as the New site dialog holds it. */
export type WordPressStarter = ReturnType<typeof useWordPressStarter>;
