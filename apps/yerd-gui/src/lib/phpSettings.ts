/**
 * Shared metadata and pure helpers for the PHP settings UI: the allowlisted
 * settings' form fields (used by the global card and each per-version panel)
 * and the inherit/override logic for per-version configuration. The daemon is
 * the validation authority; the checks here only power inline hints.
 */

/** Text-input settings of the daemon's fixed allowlist (all but display_errors). */
export const TEXT_SETTINGS = [
  {
    key: "memory_limit",
    label: "Memory limit",
    placeholder: "512M",
    hint: "Most memory one script may use. Size like 256M, 512M or 2G (use G, not GB). -1 means unlimited.",
  },
  {
    key: "max_execution_time",
    label: "Max execution time (s)",
    placeholder: "60",
    hint: "How long a script may run before it's stopped. Whole seconds, e.g. 60. 0 means no limit.",
  },
  {
    key: "max_input_time",
    label: "Max input time (s)",
    placeholder: "60",
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
    label: "Upload max filesize",
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
] as const;

export const DISPLAY_ERRORS_HINT =
  "Whether PHP shows errors in the page output. On is handy in development; Off is safer in production.";

export const DISPLAY_ERRORS_OPTIONS = [
  { value: "", label: "- default -" },
  { value: "On", label: "On" },
  { value: "Off", label: "Off" },
] as const;

/** Every allowlisted setting name, including the select-backed display_errors. */
export const SETTING_KEYS: readonly string[] = [
  ...TEXT_SETTINGS.map((s) => s.key),
  "display_errors",
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
