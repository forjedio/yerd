//! The global PHP ini settings Yerd manages, with pure validation.
//!
//! Yerd lets users set a small, fixed set of PHP runtime directives
//! (`memory_limit`, `upload_max_filesize`, …) that are written into **every**
//! installed version's FPM pool config as `php_value[...]` / `php_flag[...]`
//! lines. The values flow, unescaped, straight into the FPM master config
//! file, so [`validate_value`] is the **security boundary**: it is run when a
//! value is set (CLI + daemon), when the config is loaded from disk
//! (`yerd-config`), and again defensively at render time (`yerd-php`).
//!
//! This module is pure: an allowlist of supported directives plus per-kind
//! value validators, hand-rolled (no `regex` dependency).

use std::fmt;

use thiserror::Error;

/// Longest accepted value, in bytes. Generous for `error_reporting` constant
/// expressions while bounding the FPM config line.
const MAX_VALUE_LEN: usize = 256;

/// How a supported setting's value is validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A byte size: digits with an optional `K`/`M`/`G` suffix (any case).
    /// `allow_unlimited` additionally accepts the literal `-1`.
    Bytes { allow_unlimited: bool },
    /// A non-negative integer (`0` is allowed).
    Int,
    /// A boolean, rendered as a `php_flag` (`On`/`Off`).
    Flag,
    /// An `error_reporting` bitmask: an integer or a constant expression such
    /// as `E_ALL & ~E_DEPRECATED`.
    ErrorReporting,
}

/// One supported setting and how to validate it.
struct Spec {
    name: &'static str,
    kind: Kind,
    /// Yerd's shipped default value (canonical form). Seeded into a fresh
    /// config's `[php.settings]` and written into every FPM pool.
    default: &'static str,
    /// Whether this directive is meaningful for the CLI (`php` shim) and so
    /// belongs in the generated CLI `php.ini`. Request-only directives
    /// (upload/post sizes, input time, file uploads) are FPM-only.
    cli: bool,
}

/// The fixed allowlist. Extend here to support more directives.
const SETTINGS: &[Spec] = &[
    Spec {
        name: "memory_limit",
        kind: Kind::Bytes {
            allow_unlimited: true,
        },
        default: "512M",
        cli: true,
    },
    Spec {
        name: "max_execution_time",
        kind: Kind::Int,
        default: "60",
        cli: true,
    },
    Spec {
        name: "max_input_time",
        kind: Kind::Int,
        default: "60",
        cli: false,
    },
    Spec {
        name: "max_file_uploads",
        kind: Kind::Int,
        default: "20",
        cli: false,
    },
    Spec {
        name: "upload_max_filesize",
        kind: Kind::Bytes {
            allow_unlimited: false,
        },
        default: "100M",
        cli: false,
    },
    Spec {
        name: "post_max_size",
        kind: Kind::Bytes {
            allow_unlimited: false,
        },
        default: "100M",
        cli: false,
    },
    Spec {
        name: "display_errors",
        kind: Kind::Flag,
        default: "On",
        cli: true,
    },
    Spec {
        name: "error_reporting",
        kind: Kind::ErrorReporting,
        default: "E_ALL",
        cli: true,
    },
];

fn spec(name: &str) -> Option<&'static Spec> {
    SETTINGS.iter().find(|s| s.name == name)
}

/// Whether `name` is a setting Yerd manages.
#[must_use]
pub fn is_supported(name: &str) -> bool {
    spec(name).is_some()
}

/// The supported setting names, in declaration order (for CLI help / errors).
#[must_use]
pub fn supported_names() -> Vec<&'static str> {
    SETTINGS.iter().map(|s| s.name).collect()
}

/// Yerd's shipped opinionated defaults as `(name, canonical_value)` pairs, in
/// declaration order. Seeded into a fresh config (`yerd-config`) and the single
/// source of truth for "the values Yerd applies out of the box".
#[must_use]
pub fn default_settings() -> Vec<(&'static str, &'static str)> {
    SETTINGS.iter().map(|s| (s.name, s.default)).collect()
}

/// Whether a supported directive is meaningful for the CLI runtime and so
/// belongs in the generated CLI `php.ini`. `false` for unknown names.
#[must_use]
pub fn applies_to_cli(name: &str) -> bool {
    spec(name).is_some_and(|s| s.cli)
}

/// Render the body of the CLI `php.ini` from a set of effective settings:
/// emit only the CLI-relevant directives (`applies_to_cli`), in the allowlist's
/// declaration order, each validated and canonicalised, as `name = value` lines.
/// Unsupported or invalid entries are skipped (defensive - `validate_value` is
/// the security boundary). Empty when no CLI directives are set.
#[must_use]
pub fn render_cli_ini(settings: &std::collections::BTreeMap<String, String>) -> String {
    let mut out = String::new();
    for s in SETTINGS.iter().filter(|s| s.cli) {
        if let Some(raw) = settings.get(s.name) {
            if validate_value(s.name, raw).is_ok() {
                out.push_str(s.name);
                out.push_str(" = ");
                out.push_str(&canonical_value(s.name, raw));
                out.push('\n');
            }
        }
    }
    out
}

/// Sanitise a CA-bundle path for use as an unquoted `openssl.cafile` /
/// `curl.cainfo` value in a php.ini or FPM pool config. An unquoted ini value is
/// not escaped, so a control character breaks the line and a `;` or `#` starts a
/// comment that would truncate the directive; either would leave PHP pointed at
/// a broken path. Returns the path as a string when safe, else `None` (skip
/// emitting the directive). Also `None` for a non-UTF-8 path, which can't be
/// represented safely here.
#[must_use]
pub fn sanitize_ca_bundle_path(path: &std::path::Path) -> Option<String> {
    let s = path.to_str()?;
    if s.chars().any(|c| c.is_control() || matches!(c, ';' | '#')) {
        return None;
    }
    Some(s.to_owned())
}

/// Render the cover-shim ini: `base` (the CLI ini body) plus directives that
/// load and enable pcov from `pcov_so`. Returns `None` if `pcov_so` isn't safe
/// to emit as an unquoted ini value: the caller must treat that as a hard
/// failure, not silently run without coverage.
#[must_use]
pub fn render_cover_ini(base: &str, pcov_so: &std::path::Path) -> Option<String> {
    let path = sanitize_ca_bundle_path(pcov_so)?;
    let mut out = base.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("extension = ");
    out.push_str(&path);
    out.push_str("\npcov.enabled = 1\n");
    Some(out)
}

/// The FPM directive a setting renders as: `"php_flag"` for booleans, else
/// `"php_value"`. `None` if the setting is not supported.
#[must_use]
pub fn directive(name: &str) -> Option<&'static str> {
    spec(name).map(|s| match s.kind {
        Kind::Flag => "php_flag",
        _ => "php_value",
    })
}

/// Validate `value` for the supported setting `name`.
///
/// Enforces a global safety invariant (non-empty, not all-whitespace,
/// `≤ MAX_VALUE_LEN`, no control characters, none of the FPM/ini
/// metacharacters `[ ] = ; #`) plus a per-kind shape. This is the security
/// boundary protecting the rendered FPM config from injection.
///
/// # Errors
/// [`PhpSettingError::Unsupported`] for an unknown `name`;
/// [`PhpSettingError::InvalidValue`] for a value failing the invariant or shape.
pub fn validate_value(name: &str, value: &str) -> Result<(), PhpSettingError> {
    let spec = spec(name).ok_or_else(|| PhpSettingError::Unsupported {
        name: name.to_owned(),
    })?;
    let err = |reason| PhpSettingError::InvalidValue {
        name: name.to_owned(),
        reason,
    };

    if value.is_empty() || value.chars().all(char::is_whitespace) {
        return Err(err(ValueErrorReason::Empty));
    }
    if value.len() > MAX_VALUE_LEN {
        return Err(err(ValueErrorReason::TooLong));
    }
    for c in value.chars() {
        if c.is_control() || matches!(c, '[' | ']' | '=' | ';' | '#') {
            return Err(err(ValueErrorReason::IllegalCharacter));
        }
    }

    let t = value.trim();
    let ok = match spec.kind {
        Kind::Bytes { allow_unlimited } => is_byte_size(t, allow_unlimited),
        Kind::Int => is_uint(t),
        Kind::Flag => parse_flag(t).is_some(),
        Kind::ErrorReporting => is_error_reporting(t),
    };
    if ok {
        Ok(())
    } else {
        Err(err(match spec.kind {
            Kind::Bytes { .. } => ValueErrorReason::NotAByteSize,
            Kind::Int => ValueErrorReason::NotAnInteger,
            Kind::Flag => ValueErrorReason::NotABoolean,
            Kind::ErrorReporting => ValueErrorReason::NotErrorReporting,
        }))
    }
}

/// Normalise a (validated) value to its canonical stored/rendered form:
/// booleans become `On`/`Off`; everything else is trimmed. Unknown settings
/// are returned trimmed unchanged.
#[must_use]
pub fn canonical_value(name: &str, value: &str) -> String {
    match spec(name).map(|s| s.kind) {
        Some(Kind::Flag) => match parse_flag(value.trim()) {
            Some(true) => "On".to_owned(),
            Some(false) => "Off".to_owned(),
            None => value.trim().to_owned(),
        },
        _ => value.trim().to_owned(),
    }
}

/// `\d+` with an optional single `K`/`M`/`G` suffix (any case); plus the
/// literal `-1` when `allow_unlimited`.
fn is_byte_size(s: &str, allow_unlimited: bool) -> bool {
    if allow_unlimited && s == "-1" {
        return true;
    }
    let digits = match s.strip_suffix(['K', 'M', 'G', 'k', 'm', 'g']) {
        Some(rest) => rest,
        None => s,
    };
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// One-or-more ASCII digits.
fn is_uint(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Case-insensitive boolean: `on|off|1|0|true|false`.
fn parse_flag(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "on" | "1" | "true" => Some(true),
        "off" | "0" | "false" => Some(false),
        _ => None,
    }
}

/// An integer or a constant expression: characters limited to
/// `[A-Za-z0-9_ &|~^()-]`, with at least one non-space character.
fn is_error_reporting(s: &str) -> bool {
    s.chars().any(|c| !c.is_whitespace())
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | ' ' | '&' | '|' | '~' | '^' | '(' | ')' | '-')
        })
}

/// Failure to set or validate a managed PHP setting.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PhpSettingError {
    /// `name` is not a setting Yerd manages.
    #[error("unknown PHP setting {name:?}")]
    Unsupported {
        /// The rejected setting name.
        name: String,
    },
    /// The value failed the safety invariant or the setting's shape.
    #[error("invalid value for PHP setting {name:?}: {reason}")]
    InvalidValue {
        /// The setting whose value was rejected.
        name: String,
        /// Why the value was rejected.
        reason: ValueErrorReason,
    },
}

/// Specific failure modes for a PHP setting value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValueErrorReason {
    /// Empty or all-whitespace.
    Empty,
    /// Longer than the accepted maximum.
    TooLong,
    /// Contained a control char or an FPM/ini metacharacter (`[ ] = ; #`).
    IllegalCharacter,
    /// Not a byte size (`\d+[KMG]?`, or `-1` for `memory_limit`).
    NotAByteSize,
    /// Not a non-negative integer.
    NotAnInteger,
    /// Not a recognised boolean (`on|off|1|0|true|false`).
    NotABoolean,
    /// Not a valid `error_reporting` integer or constant expression.
    NotErrorReporting,
}

impl fmt::Display for ValueErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Empty => "value must not be empty",
            Self::TooLong => "value is too long",
            Self::IllegalCharacter => "value contains a control or reserved character ([ ] = ; #)",
            Self::NotAByteSize => "expected a byte size like 256M, 1G, or -1",
            Self::NotAnInteger => "expected a non-negative integer",
            Self::NotABoolean => "expected On or Off",
            Self::NotErrorReporting => {
                "expected an integer or a constant like E_ALL & ~E_DEPRECATED"
            }
        };
        f.write_str(msg)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn supported_set_and_directives() {
        assert!(is_supported("memory_limit"));
        assert!(!is_supported("allow_url_fopen"));
        assert_eq!(directive("memory_limit"), Some("php_value"));
        assert_eq!(directive("display_errors"), Some("php_flag"));
        assert_eq!(directive("nope"), None);
        assert_eq!(supported_names().len(), 8);
    }

    #[test]
    fn default_settings_cover_all_and_are_valid() {
        let defaults = default_settings();
        assert_eq!(defaults.len(), 8);
        for (name, value) in &defaults {
            assert!(validate_value(name, value).is_ok(), "{name}={value}");
        }
        assert!(defaults.contains(&("memory_limit", "512M")));
        assert!(defaults.contains(&("error_reporting", "E_ALL")));
    }

    #[test]
    fn sanitize_ca_bundle_path_accepts_clean_paths_and_rejects_ini_metachars() {
        use std::path::Path;
        assert_eq!(
            sanitize_ca_bundle_path(Path::new(
                "/Users/x/Library/Application Support/io.yerd.Yerd/cacert.pem"
            ))
            .as_deref(),
            Some("/Users/x/Library/Application Support/io.yerd.Yerd/cacert.pem")
        );
        assert!(sanitize_ca_bundle_path(Path::new("/d/ca\ncert.pem")).is_none());
        assert!(sanitize_ca_bundle_path(Path::new("/d/ca;cert.pem")).is_none());
        assert!(sanitize_ca_bundle_path(Path::new("/d/ca#cert.pem")).is_none());
    }

    #[test]
    fn render_cover_ini_appends_pcov_directives() {
        use std::path::Path;
        let pcov = Path::new("/d/pcov.so");
        assert_eq!(
            render_cover_ini("", pcov).as_deref(),
            Some("extension = /d/pcov.so\npcov.enabled = 1\n")
        );
        assert_eq!(
            render_cover_ini("memory_limit = 512M\n", pcov).as_deref(),
            Some("memory_limit = 512M\nextension = /d/pcov.so\npcov.enabled = 1\n")
        );
    }

    #[test]
    fn render_cover_ini_inserts_separator_when_base_lacks_trailing_newline() {
        use std::path::Path;
        let pcov = Path::new("/d/pcov.so");
        assert_eq!(
            render_cover_ini("memory_limit = 512M", pcov).as_deref(),
            Some("memory_limit = 512M\nextension = /d/pcov.so\npcov.enabled = 1\n")
        );
    }

    #[test]
    fn render_cover_ini_appends_after_an_existing_pcov_directive() {
        use std::path::Path;
        let base = "pcov.enabled = 0\n";
        let got = render_cover_ini(base, Path::new("/d/pcov.so")).unwrap();
        assert!(got.starts_with(base));
        assert!(got.ends_with("extension = /d/pcov.so\npcov.enabled = 1\n"));
    }

    #[test]
    fn render_cover_ini_rejects_an_unsafe_pcov_path() {
        use std::path::Path;
        assert!(render_cover_ini("", Path::new("/d/pc;ov.so")).is_none());
        assert!(render_cover_ini("", Path::new("/d/pc\nov.so")).is_none());
    }

    #[test]
    fn cli_subset_and_ini_render() {
        assert!(applies_to_cli("memory_limit"));
        assert!(applies_to_cli("display_errors"));
        assert!(!applies_to_cli("upload_max_filesize"));
        assert!(!applies_to_cli("nope"));

        let settings: std::collections::BTreeMap<String, String> = default_settings()
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let ini = render_cli_ini(&settings);
        assert!(ini.contains("memory_limit = 512M\n"));
        assert!(ini.contains("display_errors = On\n"));
        assert!(!ini.contains("upload_max_filesize"));
        assert!(!ini.contains("post_max_size"));
    }

    #[test]
    fn unsupported_name_is_rejected() {
        assert!(matches!(
            validate_value("allow_url_fopen", "1"),
            Err(PhpSettingError::Unsupported { .. })
        ));
    }

    #[test]
    fn byte_sizes() {
        for v in ["256M", "1G", "512m", "1024", "100K", "-1"] {
            assert!(validate_value("memory_limit", v).is_ok(), "{v}");
        }
        for v in ["256MB", "1.5G", "M", "12X", "-5", "-1K"] {
            assert!(validate_value("memory_limit", v).is_err(), "{v}");
        }
        assert!(validate_value("upload_max_filesize", "-1").is_err());
        assert!(validate_value("upload_max_filesize", "100M").is_ok());
    }

    #[test]
    fn integers() {
        assert!(validate_value("max_execution_time", "0").is_ok());
        assert!(validate_value("max_execution_time", "300").is_ok());
        assert!(validate_value("max_file_uploads", "20").is_ok());
        assert!(validate_value("max_execution_time", "30s").is_err());
        assert!(validate_value("max_execution_time", "-1").is_err());
    }

    #[test]
    fn flags_validate_and_canonicalise() {
        for v in ["On", "off", "1", "0", "true", "FALSE"] {
            assert!(validate_value("display_errors", v).is_ok(), "{v}");
        }
        assert!(validate_value("display_errors", "yes").is_err());
        assert_eq!(canonical_value("display_errors", "on"), "On");
        assert_eq!(canonical_value("display_errors", "0"), "Off");
        assert_eq!(canonical_value("memory_limit", "  512M "), "512M");
    }

    #[test]
    fn error_reporting_accepts_constants_and_ints() {
        for v in [
            "E_ALL",
            "E_ALL & ~E_DEPRECATED & ~E_NOTICE",
            "22519",
            "E_ALL | E_STRICT",
        ] {
            assert!(validate_value("error_reporting", v).is_ok(), "{v}");
        }
        assert!(validate_value("error_reporting", "   ").is_err());
    }

    #[test]
    fn injection_attempts_are_rejected() {
        assert!(validate_value("memory_limit", "1\nuser = root").is_err());
        assert!(validate_value("memory_limit", "256M; evil").is_err());
        assert!(validate_value("error_reporting", "E_ALL # comment").is_err());
        assert!(validate_value("error_reporting", "E_ALL]").is_err());
        assert!(validate_value("memory_limit", "256M=x").is_err());
        assert!(validate_value("memory_limit", &"9".repeat(MAX_VALUE_LEN + 1)).is_err());
        assert!(validate_value("memory_limit", "   ").is_err());
        assert!(validate_value("memory_limit", "").is_err());
    }
}
