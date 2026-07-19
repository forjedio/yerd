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

/** Directive names Yerd manages elsewhere, with a hint pointing at that path. */
const RESERVED_DIRECTIVES: Record<string, string> = {
  extension: "extensions are managed in the Custom extensions panel",
  zend_extension: "extensions are managed in the Custom extensions panel",
  "openssl.cafile": "Yerd manages the CA bundle for this",
  "curl.cainfo": "Yerd manages the CA bundle for this",
};

/**
 * Client-side hint for an invalid or reserved custom-directive name; `null`
 * when it looks fine. Mirrors the daemon's `yerd-core` rules loosely - the
 * daemon remains the authority.
 */
export function directiveNameProblem(name: string): string | null {
  if (name === "") return "enter a directive name";
  if (SETTING_KEYS.includes(name)) {
    return "this setting has its own field in the settings form above";
  }
  const reserved = RESERVED_DIRECTIVES[name];
  if (reserved) return reserved;
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
