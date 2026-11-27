/**
 * Shared metadata and pure helpers for the PHP settings UI: the allowlisted
 * settings' form fields (used by the global card and each per-version panel)
 * and the inherit/override logic for per-version configuration. The daemon is
 * the validation authority; the checks here only power inline hints.
 */

import type { XdebugMode, XdebugSettings, XdebugStart } from "@/ipc/types";

/** One text-input setting of the daemon's allowlist. */
export interface TextSetting {
  key: string;
  label: string;
  placeholder: string;
  hint: string;
  /** A faint suffix drawn inside the field. */
  unit?: "sec";
}

/** Text-input settings of the daemon's fixed allowlist (all but display_errors). */
export const TEXT_SETTINGS: readonly TextSetting[] = [
  {
    key: "memory_limit",
    label: "Memory limit",
    placeholder: "512M",
    hint: "Most memory one script may use. Size like 256M, 512M or 2G (use G, not GB). -1 means unlimited.",
  },
  {
    key: "max_execution_time",
    label: "Max execution time",
    placeholder: "60",
    unit: "sec",
    hint: "How long a script may run before it's stopped. Whole seconds, e.g. 60. 0 means no limit.",
  },
  {
    key: "max_input_time",
    label: "Max input time",
    placeholder: "60",
    unit: "sec",
    hint: "How long a script may spend reading request data (POST and uploads). Whole seconds, e.g. 60.",
  },
  {
    key: "max_file_uploads",
    label: "Max file uploads",
    placeholder: "20",
    hint: "How many files may be uploaded in one request. Whole number, e.g. 20.",
  },
  {
    key: "upload_max_filesize",
    label: "Upload max file size",
    placeholder: "100M",
    hint: "Largest single uploaded file. Size like 8M, 100M or 1G (use G, not GB).",
  },
  {
    key: "post_max_size",
    label: "Post max size",
    placeholder: "100M",
    hint: "Largest POST body; set this at or above the upload size. Size like 8M, 100M or 1G (use G, not GB).",
  },
  {
    key: "error_reporting",
    label: "Error reporting",
    placeholder: "E_ALL",
    hint: "Which error levels PHP reports. An integer or a constant expression, e.g. E_ALL or E_ALL & ~E_DEPRECATED.",
  },
];

export const DISPLAY_ERRORS_HINT =
  "Whether PHP shows errors in the page output. On is handy in development; Off is safer in production.";

/** The shared display_errors choices: blank leaves PHP's own default. */
export const DISPLAY_ERRORS_OPTIONS: readonly { value: string; label: string }[] = [
  { value: "", label: "Default" },
  { value: "On", label: "On" },
  { value: "Off", label: "Off" },
];

/** The short FFI row label in the settings grid. */
export const FFI_SHORT_LABEL = "FFI in web requests";

/** The allowlisted setting that allows FFI in web requests (FPM only). */
export const FFI_KEY = "ffi.enable";

export const FFI_LABEL = "Allow FFI in web requests (needed for php-vips)";

export const FFI_HINT =
  "Lets site code call native C libraries through PHP's FFI extension. Fine for local development, which is why it's off by default. The CLI already allows FFI.";

export const LIBVIPS_HINT =
  "libvips wasn't found. macOS: brew install vips. Debian/Ubuntu: libvips42 (or libvips42t64). Fedora: vips. Arch: libvips.";

export const NO_FFI_VERSIONS_HINT =
  "None of the installed PHP versions include the FFI extension.";

/** Xdebug modes in the daemon's normalised order, with labels for the panel. */
export const XDEBUG_MODES: readonly {
  value: XdebugMode;
  label: string;
  /** The mode chip's label, where it is shorter than `label`. */
  short?: string;
  note?: string;
}[] = [
  { value: "debug", label: "Step debugging" },
  { value: "develop", label: "Development helpers" },
  { value: "coverage", label: "Code coverage" },
  { value: "profile", label: "Profiler", note: "slows every request" },
  { value: "trace", label: "Function trace", short: "Trace", note: "slows every request" },
  { value: "gcstats", label: "GC statistics", short: "GC stats" },
];

export const XDEBUG_START_OPTIONS: readonly { value: XdebugStart; label: string }[] = [
  { value: "always", label: "On every request" },
  { value: "trigger", label: "Only with a trigger (XDEBUG_TRIGGER)" },
];

/** Built-in settings a version runs with when it has no `xdebug` entry. */
export const XDEBUG_DEFAULT: XdebugSettings = {
  enabled: false,
  modes: ["debug"],
  start: "always",
};

export const XDEBUG_IDE_HINT =
  "Your IDE listens on port 9003 by default; match any xdebug.client_port override.";

/** Shown above the custom directives while this version's managed Xdebug is on. */
export const XDEBUG_DIRECTIVES_HINT =
  "While Xdebug is on, the Xdebug section sets xdebug.mode and " +
  "xdebug.start_with_request for the web pool, and opcache.jit is disabled.";

export const XDEBUG_DOCS_URL = "https://yerd.app/guide/xdebug";

export const XDEBUG_UNAVAILABLE_HINT =
  "Xdebug isn't published for this PHP version on this platform.";

/** Every allowlisted setting name, including the select-backed display_errors and ffi.enable. */
export const SETTING_KEYS: readonly string[] = [
  ...TEXT_SETTINGS.map((s) => s.key),
  "display_errors",
  FFI_KEY,
];

/**
 * The value a version effectively runs with for `key`: its override when set,
 * else the global default, else `undefined` (PHP's built-in default).
 */
export function effectiveValue(
  global: Record<string, string>,
  overrides: Record<string, string>,
  key: string,
): string | undefined {
  const o = overrides[key];
  if (o !== undefined && o !== "") return o;
  const g = global[key];
  return g !== undefined && g !== "" ? g : undefined;
}

/** How many allowlisted settings a version's override map actually overrides. */
export function overrideCount(overrides: Record<string, string>): number {
  return SETTING_KEYS.filter((k) => (overrides[k] ?? "") !== "").length;
}

/**
 * Directive names Yerd manages elsewhere, with a hint pointing at that path.
 *
 * A `Map` rather than an object literal so a directive named after something on
 * `Object.prototype` (`constructor`, `toString`, …) can't resolve up the
 * prototype chain and return a function where a hint string is expected.
 */
const RESERVED_DIRECTIVES = new Map<string, string>([
  ["extension", "load extensions in the Extra extensions card"],
  ["zend_extension", "load extensions in the Extra extensions card"],
  ["openssl.cafile", "Yerd manages the CA bundle for this"],
  ["curl.cainfo", "Yerd manages the CA bundle for this"],
]);

/**
 * Client-side hint for an invalid or reserved custom-directive name; `null`
 * when it looks fine. Mirrors the daemon's `yerd-core` rules loosely - the
 * daemon remains the authority.
 */
export function directiveNameProblem(name: string): string | null {
  if (name === "") return "enter a directive name";
  if (name === FFI_KEY) return "use the FFI in web requests setting";
  if (SETTING_KEYS.includes(name)) {
    return "this setting has its own row in the Settings grid";
  }
  const reserved = RESERVED_DIRECTIVES.get(name);
  if (reserved) return reserved;
  if (name.startsWith("pm.")) {
    return "FPM pool settings are set in the FPM workers setting";
  }
  if (!/^[A-Za-z_][A-Za-z0-9._-]*$/.test(name) || name.length > 128) {
    return "names start with a letter or _ and use letters, digits, '.', '_' or '-'";
  }
  return null;
}

/**
 * Client-side hint for an invalid custom-directive value; `null` when it looks
 * fine. Rejects the ini/FPM metacharacters and control characters the daemon
 * refuses.
 */
export function directiveValueProblem(value: string): string | null {
  if (value.trim() === "") return "enter a value";
  if (value.length > 256) return "value is too long";
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f[\]=;#]/.test(value)) {
    return "values can't contain [ ] = ; # or control characters";
  }
  return null;
}

/**
 * Client-side hint for an invalid FPM `max_children` value; `null` when it
 * looks fine. An empty field is valid and means "reset to the default", so it
 * returns `null` too. Mirrors `yerd-core`'s `1..=1024` bound - the daemon
 * remains the authority.
 */
export function poolMaxChildrenProblem(value: string): string | null {
  const trimmed = value.trim();
  if (trimmed === "") return null;
  if (!/^\d+$/.test(trimmed)) return "enter a whole number";
  const n = Number(trimmed);
  if (n < 1 || n > 1024) return "worker ceiling must be between 1 and 1024";
  return null;
}

/** The version-only FPM worker ceiling, shown as a settings-grid row. */
export const POOL_MAX_CHILDREN_KEY = "max_children";

/** The worker ceiling a version runs with when it has no override. */
export const POOL_DEFAULT = "16";

/** Tooltip for the FPM workers row. */
export const POOL_HINT =
  "Most PHP workers this version may run at once (pm.max_children, 1-1024). The pool is on-demand, so a higher ceiling costs nothing while idle. Raise it if requests queue behind queue workers, parallel tests, or many open tabs.";

/**
 * The settings grid's row order, as drawn: the shared allowlist first, then the
 * version-only rows (the worker ceiling and FFI). Callers drop the rows a pane
 * doesn't show.
 */
export const GRID_ORDER: readonly string[] = [
  "memory_limit",
  "upload_max_filesize",
  "post_max_size",
  "max_execution_time",
  "max_file_uploads",
  "max_input_time",
  "display_errors",
  "error_reporting",
  POOL_MAX_CHILDREN_KEY,
  FFI_KEY,
];

/**
 * How many of a version's grid rows are changed from the shared defaults: every
 * non-empty allowlisted value plus a non-empty worker ceiling. An FFI override
 * is ignored when the build lacks FFI, since its row is hidden.
 */
export function changedCount(
  form: Record<string, string>,
  poolValue: string,
  ffiAvailable: boolean,
): number {
  const settings = SETTING_KEYS.filter(
    (k) => (ffiAvailable || k !== FFI_KEY) && (form[k] ?? "").trim() !== "",
  ).length;
  return settings + (poolValue.trim() !== "" ? 1 : 0);
}

/**
 * A version's display_errors choices: On and Off when the shared defaults set a
 * value (picking it again means inherit), else Default, On and Off.
 */
export function displayErrorsOptions(
  inherited: string | undefined,
): { value: string; label: string }[] {
  const onOff = [
    { value: "On", label: "On" },
    { value: "Off", label: "Off" },
  ];
  return inherited ? onOff : [{ value: "", label: "Default" }, ...onOff];
}

/** The override to stage for a picked display_errors value: blank when it matches the inherited one. */
export function displayErrorsPick(picked: string, inherited: string | undefined): string {
  return inherited !== undefined && picked === inherited ? "" : picked;
}
