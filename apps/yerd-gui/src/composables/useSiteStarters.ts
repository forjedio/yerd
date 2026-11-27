import { computed, type ComputedRef, type UnwrapNestedRefs } from "vue";

import type { SiteBasicsForm } from "@/composables/useSiteBasics";
import type { PrereqRow, ToolPrereqs } from "@/composables/useToolPrereqs";
import { comparePhpVersions } from "@/lib/phpVersion";
import type { ComposerFramework, CreateSiteSpec, StatusReport } from "@/ipc/types";

/** Which starter the New site dialog creates from. */
export type StarterId = "laravel" | "wordpress" | ComposerFramework;

/** What every starter reads from the New site dialog. */
export interface StarterContext {
  phpVersions: () => string[];
  defaultPhp: () => string;
  /** The dialog's one toolchain instance, shared by every starter. */
  prereqs: ToolPrereqs;
  report: () => StatusReport | null;
}

/**
 * The common shape the New site dialog drives, whichever starter is picked: the
 * PHP window, the toolchain gate, the option form's validity, the create spec
 * and the Done screen's summary. `reset()` restores only the starter's own
 * options; the shared Project form belongs to the dialog.
 */
export interface StarterModel {
  id: StarterId;
  label: string;
  /** The daemon's phase names for this starter's job, in order. */
  phases: string[];
  supportsPhp(v: string): boolean;
  /** The PHP to preselect, or "" when no supported version is installed. */
  preferredPhp(): string;
  /** The PHP picker's label for `v` (WordPress marks legacy versions). */
  phpLabel(v: string): string;
  /** The PHP picker's tooltip for the current pick. */
  phpHint(php: string): string;
  /** Shown in place of the PHP picker when no usable version is installed. */
  noPhpText: string;
  prereqRows: ComputedRef<PrereqRow[]>;
  /** What the missing-tools step says the starter needs. */
  prereqSummary: string;
  /** Whether the required toolchain is present. */
  ready: ComputedRef<boolean>;
  installAll(): Promise<void>;
  optionsValid: ComputedRef<boolean>;
  buildSpec(): CreateSiteSpec;
  /** The Done screen's one-line summary of what was created. */
  doneSummary: ComputedRef<string>;
  reset(): void;
}

/** A starter as the dialog holds it: wrapped in `reactive`, so its refs unwrap. */
export type ActiveStarter = UnwrapNestedRefs<StarterModel>;

/**
 * The default PHP when it is supported, else the newest supported version, so
 * the picker always opens on a valid choice ("" when none is supported).
 */
export function preferredPhp(
  versions: readonly string[],
  defaultPhp: string,
  supports: (v: string) => boolean,
): string {
  const supported = versions.filter(supports).sort(comparePhpVersions);
  if (defaultPhp && supported.includes(defaultPhp)) return defaultPhp;
  return supported[supported.length - 1] ?? "";
}

const COMPOSER_PHASES = ["Preflight", "Scaffolding", "Registering", "Done"];

/** The Composer-scaffolded starters, their package and minimum PHP. */
export const FRAMEWORKS: Record<ComposerFramework, { label: string; pkg: string; minPhp: string }> =
  {
    codeigniter: { label: "CodeIgniter", pkg: "codeigniter4/appstarter", minPhp: "8.1" },
    cakephp: { label: "CakePHP", pkg: "cakephp/app", minPhp: "8.1" },
    slim: { label: "Slim", pkg: "slim/skeleton", minPhp: "7.4" },
  };

/** A `composer create-project` starter (CodeIgniter, CakePHP, Slim): no options. */
export function useComposerStarter(
  framework: ComposerFramework,
  basics: SiteBasicsForm,
  ctx: StarterContext,
): StarterModel & { pkg: string } {
  const meta = FRAMEWORKS[framework];
  const prereqs = ctx.prereqs;

  function supportsPhp(v: string): boolean {
    return comparePhpVersions(v, meta.minPhp) >= 0;
  }
  const supportedPhpVersions = computed(() => ctx.phpVersions().filter(supportsPhp));

  const noSupportedPhp = computed(() => supportedPhpVersions.value.length === 0);
  const needsComposer = computed(() => !prereqs.available("composer"));
  const ready = computed(() => !noSupportedPhp.value && !needsComposer.value);

  function installPhp(): Promise<boolean> {
    return prereqs.installPhp(supportsPhp, `No installable PHP ${meta.minPhp} or newer was found.`);
  }

  const prereqRows = computed<PrereqRow[]>(() => [
    {
      id: "php",
      label: "PHP",
      sub:
        noSupportedPhp.value && ctx.phpVersions().length > 0
          ? `Installed, but not ${meta.minPhp}+`
          : `Runtime ${meta.minPhp}+`,
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
  ]);

  function installAll(): Promise<void> {
    return prereqs.installAll(async () => {
      if (noSupportedPhp.value && !(await installPhp())) return false;
      if (needsComposer.value && !(await prereqs.installTool("composer"))) return false;
      return true;
    });
  }

  function buildSpec(): CreateSiteSpec {
    return {
      name: basics.name.trim(),
      parent_dir: basics.location,
      php: basics.php,
      secure: basics.secure,
      framework: { framework },
    };
  }

  return {
    id: framework,
    label: meta.label,
    pkg: meta.pkg,
    phases: COMPOSER_PHASES,
    supportsPhp,
    preferredPhp: () => preferredPhp(ctx.phpVersions(), ctx.defaultPhp(), supportsPhp),
    phpLabel: (v) => v,
    phpHint: () => `The version this site runs on. ${meta.label} needs PHP ${meta.minPhp} or newer.`,
    noPhpText: "No supported PHP installed.",
    prereqRows,
    prereqSummary: `Creating a ${meta.label} site needs PHP and Composer.`,
    ready,
    installAll,
    optionsValid: computed(() => true),
    buildSpec,
    doneSummary: computed(() => `${meta.label} · PHP ${basics.php}`),
    reset: () => undefined,
  };
}
