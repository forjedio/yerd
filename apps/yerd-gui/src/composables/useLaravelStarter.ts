import { computed, reactive, ref } from "vue";

import { useResource } from "@/composables/useResource";
import type { SiteBasicsForm } from "@/composables/useSiteBasics";
import { preferredPhp, type StarterContext, type StarterModel } from "@/composables/useSiteStarters";
import type { PrereqRow } from "@/composables/useToolPrereqs";
import { useToast } from "@/composables/useToast";
import { listNode } from "@/ipc/client";
import type {
  AuthProvider,
  CreateSiteSpec,
  Database,
  JsRuntime,
  LaravelOptions,
  StarterKit,
  StarterKitTag,
  Testing,
} from "@/ipc/types";
import { laravelCommand } from "@/lib/createSite";
import { phpVersionInRange } from "@/lib/phpVersion";

const PHASES = ["Preflight", "Scaffolding", "Registering", "Done"];

/** The starter kit tiles of the Options step. */
export const KIT_OPTIONS: { value: StarterKitTag | "community"; label: string; hint: string }[] = [
  { value: "none", label: "None", hint: "Plain skeleton" },
  { value: "react", label: "React", hint: "Inertia + TS" },
  { value: "vue", label: "Vue", hint: "Inertia + TS" },
  { value: "livewire", label: "Livewire", hint: "Blade + PHP" },
  { value: "svelte", label: "Svelte", hint: "Inertia + TS" },
  { value: "community", label: "Community kit", hint: "--using <package>" },
];

/** Display names of the Database choices. */
export const DATABASE_LABELS: Record<Database, string> = {
  sqlite: "SQLite",
  mysql: "MySQL",
  mariadb: "MariaDB",
  pgsql: "PostgreSQL",
  sqlsrv: "SQL Server",
};

/**
 * Tools each frontend choice needs, mirroring the daemon's js_runtime_tools. pnpm
 * also needs Node because its build runs Vite with node.
 */
const JS_RUNTIME_TOOLS: Record<JsRuntime, { id: string; name: string }[]> = {
  npm: [{ id: "node", name: "Node" }],
  bun: [{ id: "bun", name: "Bun" }],
  pnpm: [
    { id: "node", name: "Node" },
    { id: "pnpm", name: "pnpm" },
  ],
  skip: [],
};

/** The Laravel installer only supports this window; anything outside it is hidden
 *  from the picker rather than offered and then failing mid-create. */
const LARAVEL_MIN_PHP = "8.3";
const LARAVEL_MAX_PHP = "8.5";

/**
 * The Laravel starter: kit, auth, testing, database, frontend runtime, Node,
 * git and Boost options, plus the `laravel new` command preview and the toolchain
 * gate (PHP 8.3-8.5, Composer, the Laravel installer).
 */
export function useLaravelStarter(basics: SiteBasicsForm, ctx: StarterContext) {
  const toast = useToast();
  const prereqs = ctx.prereqs;

  const form = reactive({
    communityPackage: "",
    auth: "laravel" as AuthProvider,
    livewireClassComponents: false,
    teams: false,
    js: "npm" as JsRuntime,
    testing: "pest" as Testing,
    database: "sqlite" as Database,
    git: true,
    boost: false,
    node: "",
  });

  const kitChoice = ref<StarterKitTag | "community">("none");
  const isJsKit = computed(
    () => ["react", "vue", "svelte"].includes(kitChoice.value) || kitChoice.value === "community",
  );
  const isLivewire = computed(() => kitChoice.value === "livewire");
  const hasAuthKit = computed(() => kitChoice.value !== "none");
  /** Yerd's Node releases, newest first; shares the Node page's cache. */
  const { data: nodeData, refresh: reloadNode } = useResource("node", listNode, {
    immediate: false,
  });
  const nodeReleases = computed(() => nodeData.value ?? []);

  /** npm and pnpm run on Node, and Bun builds often call Node tools too. */
  const usesNode = computed(() => form.js !== "skip");

  /** Only worth asking with two or more Yerd releases: otherwise the job uses the
   *  only release, or the system Node, exactly as it would by default. */
  const showNodePicker = computed(() => usesNode.value && nodeReleases.value.length > 1);

  const nodeOptions = computed(() => {
    const current = nodeReleases.value.find((r) => r.default);
    return [
      { value: "", label: current ? `Default (Node ${current.version})` : "Default" },
      ...nodeReleases.value.map((r) => ({ value: r.version, label: `Node ${r.version}` })),
    ];
  });

  const stackValid = computed(
    () => kitChoice.value !== "community" || form.communityPackage.trim() !== "",
  );

  function supportsPhp(v: string): boolean {
    return phpVersionInRange(v, LARAVEL_MIN_PHP, LARAVEL_MAX_PHP);
  }
  const supportedPhpVersions = computed(() => ctx.phpVersions().filter(supportsPhp));

  /** Building the managed Laravel installer needs Yerd's own Composer; an external one can't. */
  const managedComposer = computed(() => prereqs.managed("composer"));
  const needsComposer = computed(() => !prereqs.available("composer"));
  const needsInstaller = computed(() => !prereqs.available("laravel"));
  const noSupportedPhp = computed(() => supportedPhpVersions.value.length === 0);
  /** The daemon installs these during the job when missing, so they never block the dialog. */
  const autoInstallLabel = computed(() => {
    const missing = JS_RUNTIME_TOOLS[form.js].filter((t) => !prereqs.available(t.id));
    return missing.length ? missing.map((t) => t.name).join(" and ") : null;
  });
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
        noSupportedPhp.value && ctx.phpVersions().length > 0
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

  /** Every kit ships a Vite build, so the frontend choice applies to all of them. */
  function buildOptions(): LaravelOptions {
    const kit: StarterKit =
      kitChoice.value === "community"
        ? { community: form.communityPackage.trim() }
        : (kitChoice.value as StarterKitTag);
    return {
      starter_kit: kit,
      auth: hasAuthKit.value ? form.auth : "laravel",
      livewire_class_components: isLivewire.value && form.livewireClassComponents,
      teams: hasAuthKit.value && form.teams,
      testing: form.testing,
      database: form.database,
      js: form.js,
      git: form.git,
      boost: form.boost,
      ...(showNodePicker.value && form.node ? { node: form.node } : {}),
    };
  }

  function buildSpec(): CreateSiteSpec {
    return {
      name: basics.name.trim(),
      parent_dir: basics.location,
      php: basics.php,
      secure: basics.secure,
      framework: { framework: "laravel", options: buildOptions() },
    };
  }

  /** The `laravel new` line the job will run, for the "What Yerd will do" strip. */
  const command = computed(() =>
    laravelCommand(basics.name.trim().toLowerCase() || "name", buildOptions()),
  );

  /** The Node release a build will use: the pick, else Yerd's default release. */
  const buildNode = computed(() => {
    if (!usesNode.value) return null;
    if (showNodePicker.value && form.node) return form.node;
    return nodeReleases.value.find((r) => r.default)?.version ?? null;
  });

  /** "Installs Node and pnpm, builds with Node 22.11.0, serves on PHP 8.5", minus absent parts. */
  const note = computed(() => {
    const parts = [
      autoInstallLabel.value ? `installs ${autoInstallLabel.value}` : null,
      buildNode.value ? `builds with Node ${buildNode.value}` : null,
      basics.php ? `serves on PHP ${basics.php}` : null,
    ].filter((p): p is string => p !== null);
    const text = parts.join(", ");
    return text ? text.charAt(0).toUpperCase() + text.slice(1) : "";
  });

  const doneSummary = computed(() => {
    const kit = KIT_OPTIONS.find((k) => k.value === kitChoice.value);
    return [
      "Laravel",
      kitChoice.value === "none" ? null : (kit?.label ?? null),
      form.testing === "pest" ? "Pest" : "PHPUnit",
      DATABASE_LABELS[form.database],
      `PHP ${basics.php}`,
    ]
      .filter((p): p is string => p !== null)
      .join(" · ");
  });

  function reset(): void {
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
    form.node = "";
    void reloadNode();
  }

  const model: StarterModel = {
    id: "laravel",
    label: "Laravel",
    phases: PHASES,
    supportsPhp,
    preferredPhp: () => preferredPhp(ctx.phpVersions(), ctx.defaultPhp(), supportsPhp),
    phpLabel: (v) => v,
    phpHint: () =>
      `The version this site runs on. Laravel needs PHP ${LARAVEL_MIN_PHP} to ${LARAVEL_MAX_PHP}.`,
    noPhpText: "No supported PHP installed.",
    prereqRows,
    prereqSummary: "Creating a Laravel site needs PHP, Composer and the Laravel installer.",
    ready,
    installAll,
    optionsValid: stackValid,
    buildSpec,
    doneSummary,
    reset,
  };

  return {
    ...model,
    form,
    kitChoice,
    isJsKit,
    isLivewire,
    hasAuthKit,
    showNodePicker,
    nodeOptions,
    autoInstallLabel,
    command,
    note,
  };
}

/** The Laravel starter as the New site dialog holds it. */
export type LaravelStarter = ReturnType<typeof useLaravelStarter>;
