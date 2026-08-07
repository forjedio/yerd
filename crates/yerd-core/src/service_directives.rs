//! Pure validation for free-form service configuration overrides.
//!
//! Yerd regenerates each config-backed service's own config file on every
//! start, so a hand edit there is clobbered. Instead, users set overrides that
//! Yerd renders into a sidecar file (`conf.d/10-yerd.<ext>`) the engine
//! includes after Yerd's own settings. These are not typed: Yerd cannot know
//! the grammar of every directive of every engine, so a valid-shaped but
//! nonsensical entry is the engine's problem, not Yerd's. What this module does
//! guarantee is that an override can never corrupt a generated config file:
//! [`validate_name`] and [`validate_value`] are the **injection boundary**, run
//! when an override is set (CLI + daemon), when the config is loaded from disk
//! (`yerd-config`, leniently: bad entries are dropped), and again defensively
//! at render time ([`render_managed`]).
//!
//! Every engine spells its option file differently, so the shape rules,
//! rendering, and denylist are keyed by [`OverrideDialect`]; [`dialect_for`]
//! maps a service type id onto one, and is the single source of truth for
//! "does this service accept overrides at all".
//!
//! A per-dialect denylist ([`reserved`]) keeps overrides from colliding with
//! the directives Yerd manages through typed paths (the port, the data
//! directory, the socket, logging, the bootstrap init file, and the
//! loopback-only binding).
//!
//! This module is pure: string validation and rendering only, hand-rolled
//! (no `regex` dependency), mirroring [`crate::php_directives`].

use std::collections::BTreeMap;
use std::fmt;

use thiserror::Error;

/// Longest accepted override name, in bytes. Real directive names are short
/// (`unix_socket_directories` is 23 bytes); 128 is a generous bound.
const MAX_NAME_LEN: usize = 128;

/// Longest accepted override value, in bytes. Twice
/// [`crate::php_directives`]' cap: a full `sql_mode` or `optimizer_switch`
/// list legitimately runs past 256 bytes.
const MAX_VALUE_LEN: usize = 512;

/// The option-file grammar of a service that accepts configuration overrides.
///
/// The dialect is the whole capability descriptor: the line form, the sidecar
/// file extension ([`file_ext`]) and the engine's native include directive are
/// all total functions of it, so they cannot disagree with one another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideDialect {
    /// `MySQL` / `MariaDB` option file: `name = value` lines under a
    /// `[mysqld]` group header.
    MyCnf,
    /// `postgresql.conf`: `name = 'value'` lines, single-quoted with `''`
    /// escaping.
    PostgresConf,
    /// Redis / Valkey config: bare `name value` lines.
    RedisConf,
}

/// The dialect a service type accepts overrides in, or `None` when it accepts
/// none at all (Meilisearch and Reverb are argv/env driven, and an unknown id
/// is never override-capable).
///
/// `type_id` is the service type part of a wire id, matching how
/// `ServicesSection` is keyed. This lives here rather than in `yerd-services`
/// so `yerd-config` and the CLI can learn the capability without depending on
/// that crate.
#[must_use]
pub fn dialect_for(type_id: &str) -> Option<OverrideDialect> {
    match type_id {
        "mysql" | "mariadb" => Some(OverrideDialect::MyCnf),
        "postgres" => Some(OverrideDialect::PostgresConf),
        "redis" => Some(OverrideDialect::RedisConf),
        _ => None,
    }
}

/// File extension for a dialect's sidecar override files.
///
/// `MyCnf` is `cnf` because `!includedir` reads only `*.cnf` on Unix; the
/// other two are plain `conf`.
#[must_use]
pub const fn file_ext(dialect: OverrideDialect) -> &'static str {
    match dialect {
        OverrideDialect::MyCnf => "cnf",
        OverrideDialect::PostgresConf | OverrideDialect::RedisConf => "conf",
    }
}

/// `MySQL`/`MariaDB` directives Yerd owns, paired with a hint naming the typed
/// path that manages each. Spelled with `-`; matching normalises `-` and `_`.
const RESERVED_MY_CNF: &[(&str, &str)] = &[
    (
        "port",
        "the port is managed with `yerd service set-port <service>`",
    ),
    ("datadir", "Yerd owns this service's data directory"),
    ("socket", "Yerd owns this service's socket path"),
    ("pid-file", "Yerd owns this service's pid file"),
    (
        "log-error",
        "Yerd owns the log path: read it with `yerd service logs <service>`",
    ),
    (
        "init-file",
        "Yerd runs its own bootstrap SQL from this file (passwordless root over loopback)",
    ),
    ("bind-address", "Yerd pins this service to loopback"),
    ("skip-name-resolve", "Yerd pins this service to loopback"),
];

/// `PostgreSQL` directives Yerd owns, paired with a hint naming the typed path
/// that manages each.
const RESERVED_POSTGRES_CONF: &[(&str, &str)] = &[
    (
        "port",
        "the port is managed with `yerd service set-port <service>`",
    ),
    ("listen_addresses", "Yerd pins this service to loopback"),
    (
        "unix_socket_directories",
        "Yerd owns this service's socket configuration",
    ),
    (
        "logging_collector",
        "Yerd owns the log path: read it with `yerd service logs <service>`",
    ),
    (
        "hba_file",
        "Yerd pins authentication to the data directory it initialised",
    ),
    (
        "ident_file",
        "Yerd pins authentication to the data directory it initialised",
    ),
    (
        "shared_preload_libraries",
        "Yerd resolves preloaded libraries from the install tree",
    ),
    ("data_directory", "Yerd owns this service's data directory"),
    ("config_file", "Yerd owns this service's config file"),
    ("include", INCLUDE_HINT),
    ("include_dir", INCLUDE_HINT),
    ("include_if_exists", INCLUDE_HINT),
];

/// Redis/Valkey directives Yerd owns, paired with a hint naming the typed path
/// that manages each.
const RESERVED_REDIS_CONF: &[(&str, &str)] = &[
    (
        "port",
        "the port is managed with `yerd service set-port <service>`",
    ),
    ("bind", "Yerd pins this service to loopback"),
    ("protected-mode", "Yerd pins this service to loopback"),
    ("dir", "Yerd owns this service's data directory"),
    (
        "logfile",
        "Yerd owns the log path: read it with `yerd service logs <service>`",
    ),
    (
        "daemonize",
        "Yerd supervises this service in the foreground",
    ),
    ("include", INCLUDE_HINT),
    ("unixsocket", "Yerd owns this service's socket path"),
];

/// Shared hint for the include directives: chain-loading further files from a
/// managed override would reopen the injection boundary this module closes.
const INCLUDE_HINT: &str =
    "overrides may not load other config files; put hand edits in the 50-local file instead";

/// If `name` is a directive Yerd manages itself for this dialect, the
/// human-readable hint explaining where; `None` when the name is free for
/// custom use.
///
/// Covers the port, data directory, socket, pid file, logging, the
/// `MySQL`-family bootstrap init file, the loopback-only binding, and the
/// include directives. Directives Yerd merely happens to render a default for
/// are deliberately absent: an override is allowed to replace those.
///
/// `MyCnf` matching normalises `-` and `_`, which `mysqld` treats
/// interchangeably, so `bind_address` cannot slip past the `bind-address`
/// entry.
#[must_use]
pub fn reserved(dialect: OverrideDialect, name: &str) -> Option<&'static str> {
    let table = match dialect {
        OverrideDialect::MyCnf => RESERVED_MY_CNF,
        OverrideDialect::PostgresConf => RESERVED_POSTGRES_CONF,
        OverrideDialect::RedisConf => RESERVED_REDIS_CONF,
    };
    let found = match dialect {
        OverrideDialect::MyCnf => {
            let needle = name.replace('-', "_");
            table.iter().find(|(n, _)| n.replace('-', "_") == needle)
        }
        OverrideDialect::PostgresConf | OverrideDialect::RedisConf => {
            table.iter().find(|(n, _)| *n == name)
        }
    };
    found.map(|(_, hint)| *hint)
}

/// Validate an override name: non-empty, bounded, first character `[A-Za-z_]`,
/// remaining characters `[A-Za-z0-9._-]`. This accepts every real directive
/// shape across the three dialects (`max_connections`, `protected-mode`,
/// `innodb_buffer_pool_size`) while keeping a name safe on the left of an
/// unescaped `name = value` line: `!` cannot start a name, so `!include`
/// injection is impossible, and `[` is rejected outright, so a group header
/// cannot be forged.
///
/// Reserved names ([`reserved`]) are not rejected here; callers on the set path
/// check that separately so they can surface the specific hint.
///
/// # Errors
/// [`OverrideError::Name`] with the specific [`OverrideNameErrorReason`].
pub fn validate_name(name: &str) -> Result<(), OverrideError> {
    let err = |reason| Err(OverrideError::Name { reason });
    let Some(first) = name.chars().next() else {
        return err(OverrideNameErrorReason::Empty);
    };
    if name.len() > MAX_NAME_LEN {
        return err(OverrideNameErrorReason::TooLong);
    }
    if !(first.is_ascii_alphabetic() || first == '_') {
        return err(OverrideNameErrorReason::IllegalStart);
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return err(OverrideNameErrorReason::IllegalCharacter);
    }
    Ok(())
}

/// Validate an override value for a dialect: non-empty, not all-whitespace,
/// `≤ MAX_VALUE_LEN`, no control characters (which would break the value out
/// of its line), and no `#` or `;` (comment starters in these files).
///
/// Spaces and `=` are allowed: `optimizer_switch = index_merge=on` and Redis'
/// `save 900 1` are ordinary values.
///
/// Quotes are the one per-dialect rule. `MyCnf` and `RedisConf` values render
/// bare, and Valkey's argument splitter aborts the *entire* config load on an
/// unbalanced quote, so both `'` and `"` are refused there rather than risk a
/// service that will not start. `PostgresConf` values always render
/// single-quoted with `''` escaping, so a quote stays inert and is accepted.
///
/// # Errors
/// [`OverrideError::Value`] with the specific [`OverrideValueErrorReason`].
pub fn validate_value(dialect: OverrideDialect, value: &str) -> Result<(), OverrideError> {
    let err = |reason| Err(OverrideError::Value { reason });
    if value.is_empty() || value.chars().all(char::is_whitespace) {
        return err(OverrideValueErrorReason::Empty);
    }
    if value.len() > MAX_VALUE_LEN {
        return err(OverrideValueErrorReason::TooLong);
    }
    if value
        .chars()
        .any(|c| c.is_control() || matches!(c, ';' | '#'))
    {
        return err(OverrideValueErrorReason::IllegalCharacter);
    }
    let quoted = match dialect {
        OverrideDialect::MyCnf | OverrideDialect::RedisConf => {
            value.chars().any(|c| matches!(c, '\'' | '"'))
        }
        OverrideDialect::PostgresConf => false,
    };
    if quoted {
        return err(OverrideValueErrorReason::Quote);
    }
    Ok(())
}

/// Render the managed sidecar (`10-yerd.<ext>`) body for a dialect: a header
/// comment, a `[mysqld]` group line for `MyCnf` (an included option file is a
/// full option file and needs its own group), then one line per override in
/// map (alphabetical) order.
///
/// Entries failing [`validate_name`] / [`validate_value`] or naming a
/// [`reserved`] directive are skipped defensively - this renderer runs after
/// the set-time and load-time checks, so nothing malformed can reach a
/// generated file even if a bad entry slips through. Values are trimmed.
#[must_use]
pub fn render_managed(dialect: OverrideDialect, overrides: &BTreeMap<String, String>) -> String {
    let ext = file_ext(dialect);
    let mut out = format!(
        "# Managed by Yerd — regenerated on every start; do not edit by hand.\n\
         # Set these with `yerd service set <service> <key> <value>`.\n\
         # Your own edits belong in 50-local.{ext} beside this file.\n"
    );
    if dialect == OverrideDialect::MyCnf {
        out.push_str("[mysqld]\n");
    }
    for (name, value) in overrides {
        if validate_name(name).is_err()
            || validate_value(dialect, value).is_err()
            || reserved(dialect, name).is_some()
        {
            continue;
        }
        let value = value.trim();
        out.push_str(name);
        match dialect {
            OverrideDialect::MyCnf => {
                out.push_str(" = ");
                out.push_str(value);
            }
            OverrideDialect::PostgresConf => {
                out.push_str(" = '");
                out.push_str(&value.replace('\'', "''"));
                out.push('\'');
            }
            OverrideDialect::RedisConf => {
                out.push(' ');
                out.push_str(value);
            }
        }
        out.push('\n');
    }
    out
}

/// Render the one-time hand-edit sidecar (`50-local.<ext>`) body for a
/// dialect: an all-comment explanation of who owns the file and how it is
/// read, plus an uncommented `[mysqld]` group line for `MyCnf` (a directive
/// under no group applies to nothing).
///
/// Yerd writes this only when the file does not exist, so everything below the
/// header is the user's. The per-dialect closing note carries the caveat that
/// include ordering cannot deliver: last-wins holds for scalar directives, but
/// `plugin-load-add` and Redis' `save` accumulate instead of replacing, and for
/// `PostgresConf` an `ALTER SYSTEM` write outranks every included file.
#[must_use]
pub fn render_local_stub(dialect: OverrideDialect) -> String {
    let ext = file_ext(dialect);
    let (syntax, notes) = match dialect {
        OverrideDialect::MyCnf => (
            "name = value",
            "# Accumulating directives (plugin-load-add) append rather than replace,\n\
             # so setting one here adds to Yerd's value.\n\
             \n\
             [mysqld]\n",
        ),
        OverrideDialect::PostgresConf => (
            "name = value",
            "# postgresql.auto.conf (written by ALTER SYSTEM) is read after every\n\
             # include, so a directive set there outranks this file.\n",
        ),
        OverrideDialect::RedisConf => (
            "name value",
            "# Accumulating directives (save, client-output-buffer-limit) append\n\
             # rather than replace, so setting one here adds to Yerd's value.\n",
        ),
    };
    format!(
        "# Your own overrides. This file belongs to you: Yerd created it once and\n\
         # never rewrites it, so hand edits survive restarts.\n\
         #\n\
         # It is read after Yerd's own settings and after 10-yerd.{ext}, so a\n\
         # directive here wins. One directive per line, `{syntax}`.\n\
         #\n\
         # Restart the service to apply: `yerd service restart <service>`\n\
         #\n\
         # Directives Yerd manages itself (the port, data directory, socket,\n\
         # logging, loopback binding) are still refused here: `yerd doctor`\n\
         # flags them.\n\
         {notes}"
    )
}

/// Scan a hand-edited `50-local.<ext>` file for entries `yerd doctor` should
/// flag: a directive Yerd manages itself, or a line that is not a directive at
/// all for this dialect.
///
/// Blank lines, comments (`#`, plus `;` for `MyCnf`), and `MyCnf` group
/// headers and `!include` lines are skipped. Semantic validity is still the
/// engine's problem: this reports shape, not meaning.
#[must_use]
pub fn scan_local(dialect: OverrideDialect, content: &str) -> Vec<LocalOverrideIssue> {
    let mut issues = Vec::new();
    for (index, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if dialect == OverrideDialect::MyCnf
            && (line.starts_with(';') || line.starts_with('[') || line.starts_with('!'))
        {
            continue;
        }
        let line_number = index + 1;
        let Some(key) = local_line_key(dialect, line) else {
            issues.push(LocalOverrideIssue {
                line: line_number,
                key: None,
                problem: LocalOverrideProblem::Malformed,
            });
            continue;
        };
        if let Some(hint) = reserved(dialect, key) {
            issues.push(LocalOverrideIssue {
                line: line_number,
                key: Some(key.to_owned()),
                problem: LocalOverrideProblem::Reserved { hint },
            });
        }
    }
    issues
}

/// The directive name on a non-comment line of a hand-edited file, or `None`
/// when the line does not read as a directive for this dialect.
///
/// `MyCnf` accepts a valueless line because bare flags (`skip-name-resolve`)
/// are ordinary there; `PostgresConf` accepts either `name = value` or
/// `name value`, both of which its parser reads.
fn local_line_key(dialect: OverrideDialect, line: &str) -> Option<&str> {
    let (key, value) = match dialect {
        OverrideDialect::MyCnf => line
            .split_once('=')
            .map_or((line, ""), |(key, value)| (key.trim(), value.trim())),
        OverrideDialect::PostgresConf => {
            let (key, value) = line
                .split_once('=')
                .or_else(|| line.split_once(char::is_whitespace))?;
            (key.trim(), value.trim())
        }
        OverrideDialect::RedisConf => {
            let (key, value) = line.split_once(char::is_whitespace)?;
            (key.trim(), value.trim())
        }
    };
    if validate_name(key).is_err() {
        return None;
    }
    if value.is_empty() && dialect != OverrideDialect::MyCnf {
        return None;
    }
    Some(key)
}

/// One thing wrong with a line of a hand-edited `50-local.<ext>` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalOverrideIssue {
    /// 1-based line number, as an editor shows it.
    pub line: usize,
    /// The offending directive name, when the line yielded one. `None` for a
    /// line with no readable name.
    pub key: Option<String>,
    /// What is wrong with the line.
    pub problem: LocalOverrideProblem,
}

/// Why a line of a hand-edited override file was flagged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalOverrideProblem {
    /// Names a directive Yerd manages itself.
    Reserved {
        /// The [`reserved`] hint naming the typed path that manages it.
        hint: &'static str,
    },
    /// Not a directive line for this dialect.
    Malformed,
}

impl fmt::Display for LocalOverrideProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reserved { hint } => write!(f, "this directive is managed by Yerd: {hint}"),
            Self::Malformed => f.write_str("this line is not a directive"),
        }
    }
}

/// Failure to validate a service configuration override.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OverrideError {
    /// The override name was rejected.
    #[error("invalid config override name: {reason}")]
    Name {
        /// Why the name was rejected.
        reason: OverrideNameErrorReason,
    },
    /// The override value was rejected.
    #[error("invalid config override value: {reason}")]
    Value {
        /// Why the value was rejected.
        reason: OverrideValueErrorReason,
    },
}

/// Specific failure modes for an override name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OverrideNameErrorReason {
    /// Empty string.
    Empty,
    /// Longer than the accepted maximum.
    TooLong,
    /// First character was not a letter or underscore.
    IllegalStart,
    /// Contained a character outside `[A-Za-z0-9._-]`.
    IllegalCharacter,
}

impl fmt::Display for OverrideNameErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Empty => "name must not be empty",
            Self::TooLong => "name is too long",
            Self::IllegalStart => "name must start with a letter or '_'",
            Self::IllegalCharacter => "name may only contain letters, digits, '.', '_' and '-'",
        };
        f.write_str(msg)
    }
}

/// Specific failure modes for an override value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OverrideValueErrorReason {
    /// Empty or all-whitespace.
    Empty,
    /// Longer than the accepted maximum.
    TooLong,
    /// Contained a control character or a comment starter (`;`, `#`).
    IllegalCharacter,
    /// Contained a quote in a dialect whose values render bare.
    Quote,
}

impl fmt::Display for OverrideValueErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Empty => "value must not be empty",
            Self::TooLong => "value is too long",
            Self::IllegalCharacter => "value may not contain control characters, ';' or '#'",
            Self::Quote => "value may not contain quote characters",
        };
        f.write_str(msg)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const DIALECTS: [OverrideDialect; 3] = [
        OverrideDialect::MyCnf,
        OverrideDialect::PostgresConf,
        OverrideDialect::RedisConf,
    ];

    #[test]
    fn dialect_is_known_for_config_backed_services_only() {
        let cases: &[(&str, Option<OverrideDialect>)] = &[
            ("mysql", Some(OverrideDialect::MyCnf)),
            ("mariadb", Some(OverrideDialect::MyCnf)),
            ("postgres", Some(OverrideDialect::PostgresConf)),
            ("redis", Some(OverrideDialect::RedisConf)),
            ("meilisearch", None),
            ("reverb", None),
            ("", None),
            ("MySQL", None),
            ("postgresql", None),
        ];
        for (type_id, want) in cases {
            assert_eq!(dialect_for(type_id), *want, "{type_id:?}");
        }
    }

    #[test]
    fn file_ext_is_cnf_only_for_the_mysql_family() {
        assert_eq!(file_ext(OverrideDialect::MyCnf), "cnf");
        assert_eq!(file_ext(OverrideDialect::PostgresConf), "conf");
        assert_eq!(file_ext(OverrideDialect::RedisConf), "conf");
    }

    #[test]
    fn valid_names_pass() {
        for name in [
            "max_connections",
            "innodb_buffer_pool_size",
            "protected-mode",
            "maxmemory",
            "work_mem",
            "_private",
            "a",
        ] {
            assert!(validate_name(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn invalid_names_are_rejected() {
        let cases: &[(&str, OverrideNameErrorReason)] = &[
            ("", OverrideNameErrorReason::Empty),
            ("1st", OverrideNameErrorReason::IllegalStart),
            ("-dash", OverrideNameErrorReason::IllegalStart),
            ("!includedir", OverrideNameErrorReason::IllegalStart),
            ("[mysqld]", OverrideNameErrorReason::IllegalStart),
            ("has space", OverrideNameErrorReason::IllegalCharacter),
            ("semi;colon", OverrideNameErrorReason::IllegalCharacter),
            ("brack[et", OverrideNameErrorReason::IllegalCharacter),
            ("eq=uals", OverrideNameErrorReason::IllegalCharacter),
            ("new\nline", OverrideNameErrorReason::IllegalCharacter),
        ];
        for (name, want) in cases {
            assert!(
                matches!(validate_name(name), Err(OverrideError::Name { reason }) if reason == *want),
                "{name:?}"
            );
        }
        assert!(matches!(
            validate_name(&"x".repeat(MAX_NAME_LEN + 1)),
            Err(OverrideError::Name {
                reason: OverrideNameErrorReason::TooLong
            })
        ));
    }

    #[test]
    fn valid_values_pass_in_every_dialect() {
        for dialect in DIALECTS {
            for value in [
                "500",
                "256M",
                "index_merge=on,index_merge_union=off",
                "900 1",
                "/var/lib/data",
                "ON",
            ] {
                assert!(
                    validate_value(dialect, value).is_ok(),
                    "{dialect:?} {value}"
                );
            }
        }
        assert!(validate_value(OverrideDialect::MyCnf, &"9".repeat(MAX_VALUE_LEN)).is_ok());
    }

    #[test]
    fn invalid_values_are_rejected_in_every_dialect() {
        let cases: &[(&str, OverrideValueErrorReason)] = &[
            ("", OverrideValueErrorReason::Empty),
            ("   ", OverrideValueErrorReason::Empty),
            ("a\nb", OverrideValueErrorReason::IllegalCharacter),
            ("a\rb", OverrideValueErrorReason::IllegalCharacter),
            ("a;b", OverrideValueErrorReason::IllegalCharacter),
            ("a#b", OverrideValueErrorReason::IllegalCharacter),
        ];
        for dialect in DIALECTS {
            for (value, want) in cases {
                assert!(
                    matches!(validate_value(dialect, value), Err(OverrideError::Value { reason }) if reason == *want),
                    "{dialect:?} {value:?}"
                );
            }
            assert!(matches!(
                validate_value(dialect, &"9".repeat(MAX_VALUE_LEN + 1)),
                Err(OverrideError::Value {
                    reason: OverrideValueErrorReason::TooLong
                })
            ));
        }
    }

    #[test]
    fn quotes_are_rejected_except_in_postgres_values() {
        for value in ["it's", "\"quoted\"", "a'b\"c"] {
            for dialect in [OverrideDialect::MyCnf, OverrideDialect::RedisConf] {
                assert!(
                    matches!(
                        validate_value(dialect, value),
                        Err(OverrideError::Value {
                            reason: OverrideValueErrorReason::Quote
                        })
                    ),
                    "{dialect:?} {value:?}"
                );
            }
            assert!(
                validate_value(OverrideDialect::PostgresConf, value).is_ok(),
                "{value:?}"
            );
        }
    }

    #[test]
    fn every_reserved_name_has_a_hint() {
        let cases: &[(OverrideDialect, &[&str])] = &[
            (
                OverrideDialect::MyCnf,
                &[
                    "port",
                    "datadir",
                    "socket",
                    "pid-file",
                    "log-error",
                    "init-file",
                    "bind-address",
                    "skip-name-resolve",
                ],
            ),
            (
                OverrideDialect::PostgresConf,
                &[
                    "port",
                    "listen_addresses",
                    "unix_socket_directories",
                    "logging_collector",
                    "hba_file",
                    "ident_file",
                    "shared_preload_libraries",
                    "data_directory",
                    "config_file",
                    "include",
                    "include_dir",
                    "include_if_exists",
                ],
            ),
            (
                OverrideDialect::RedisConf,
                &[
                    "port",
                    "bind",
                    "protected-mode",
                    "dir",
                    "logfile",
                    "daemonize",
                    "include",
                    "unixsocket",
                ],
            ),
        ];
        for (dialect, names) in cases {
            for name in *names {
                let hint = reserved(*dialect, name).unwrap_or_default();
                assert!(!hint.is_empty(), "{dialect:?} {name}");
            }
        }
    }

    #[test]
    fn settable_names_are_not_reserved() {
        let cases: &[(OverrideDialect, &[&str])] = &[
            (
                OverrideDialect::MyCnf,
                &[
                    "max_connections",
                    "innodb_buffer_pool_size",
                    "sql_mode",
                    "bind",
                    "logfile",
                ],
            ),
            (
                OverrideDialect::PostgresConf,
                &["work_mem", "max_connections", "datadir", "bind-address"],
            ),
            (
                OverrideDialect::RedisConf,
                &[
                    "save",
                    "appendonly",
                    "maxmemory",
                    "maxmemory-policy",
                    "datadir",
                    "listen_addresses",
                ],
            ),
        ];
        for (dialect, names) in cases {
            for name in *names {
                assert!(reserved(*dialect, name).is_none(), "{dialect:?} {name}");
            }
        }
    }

    #[test]
    fn mycnf_reservations_ignore_dash_underscore_spelling() {
        for name in [
            "bind-address",
            "bind_address",
            "pid_file",
            "pid-file",
            "log_error",
            "init_file",
            "skip_name_resolve",
        ] {
            assert!(reserved(OverrideDialect::MyCnf, name).is_some(), "{name}");
        }
        assert!(reserved(OverrideDialect::PostgresConf, "listen-addresses").is_none());
        assert!(reserved(OverrideDialect::RedisConf, "protected_mode").is_none());
    }

    #[test]
    fn render_managed_uses_the_dialect_line_form() {
        let overrides = BTreeMap::from([
            ("max_connections".to_owned(), "500".to_owned()),
            ("maxmemory".to_owned(), " 256mb ".to_owned()),
        ]);
        let my = render_managed(OverrideDialect::MyCnf, &overrides);
        assert!(my.contains("[mysqld]\n"), "{my}");
        assert!(my.contains("max_connections = 500\n"), "{my}");
        assert!(my.contains("maxmemory = 256mb\n"), "{my}");

        let redis = render_managed(OverrideDialect::RedisConf, &overrides);
        assert!(!redis.contains("[mysqld]"), "{redis}");
        assert!(redis.contains("maxmemory 256mb\n"), "{redis}");

        let pg = render_managed(OverrideDialect::PostgresConf, &overrides);
        assert!(pg.contains("max_connections = '500'\n"), "{pg}");
    }

    #[test]
    fn render_managed_single_quotes_are_doubled_for_postgres() {
        let overrides = BTreeMap::from([("search_path".to_owned(), "it's mine".to_owned())]);
        let pg = render_managed(OverrideDialect::PostgresConf, &overrides);
        assert!(pg.contains("search_path = 'it''s mine'\n"), "{pg}");
    }

    #[test]
    fn render_managed_skips_invalid_and_reserved_entries() {
        let overrides = BTreeMap::from([
            ("max_connections".to_owned(), "500".to_owned()),
            ("bind_address".to_owned(), "0.0.0.0".to_owned()),
            ("datadir".to_owned(), "/tmp/x".to_owned()),
            ("bad name".to_owned(), "1".to_owned()),
            ("bad_value".to_owned(), "a\nb".to_owned()),
            ("quoted".to_owned(), "\"x".to_owned()),
        ]);
        let rendered = render_managed(OverrideDialect::MyCnf, &overrides);
        let directives: Vec<&str> = rendered
            .lines()
            .filter(|l| !l.starts_with('#') && !l.starts_with('['))
            .collect();
        assert_eq!(directives, vec!["max_connections = 500"]);
    }

    #[test]
    fn render_managed_of_an_empty_map_is_header_only() {
        for dialect in DIALECTS {
            let rendered = render_managed(dialect, &BTreeMap::new());
            assert!(
                rendered
                    .lines()
                    .all(|l| l.starts_with('#') || l == "[mysqld]"),
                "{dialect:?} {rendered}"
            );
            assert!(rendered.starts_with("# Managed by Yerd"), "{dialect:?}");
        }
    }

    #[test]
    fn local_stub_is_all_comments_apart_from_the_mysql_group() {
        for dialect in DIALECTS {
            let stub = render_local_stub(dialect);
            for line in stub.lines().filter(|l| !l.trim().is_empty()) {
                let allowed = line.starts_with('#')
                    || (dialect == OverrideDialect::MyCnf && line == "[mysqld]");
                assert!(allowed, "{dialect:?}: {line}");
            }
            assert!(stub.contains("yerd service restart"), "{dialect:?}");
            assert!(stub.contains("never rewrites it"), "{dialect:?}");
        }
        assert!(render_local_stub(OverrideDialect::MyCnf).contains("\n[mysqld]\n"));
        assert!(render_local_stub(OverrideDialect::PostgresConf).contains("postgresql.auto.conf"));
        assert!(render_local_stub(OverrideDialect::RedisConf).contains("save"));
    }

    #[test]
    fn scan_local_accepts_a_clean_file() {
        let cases: &[(OverrideDialect, &str)] = &[
            (
                OverrideDialect::MyCnf,
                "# a comment\n; another\n[mysqld]\n!includedir /x\n\nmax_connections = 500\nskip-external-locking\n",
            ),
            (
                OverrideDialect::PostgresConf,
                "# a comment\n\nwork_mem = 8MB\nmax_connections 200\n",
            ),
            (
                OverrideDialect::RedisConf,
                "# a comment\n\nmaxmemory 256mb\nsave 900 1\n",
            ),
        ];
        for (dialect, content) in cases {
            assert!(scan_local(*dialect, content).is_empty(), "{dialect:?}");
        }
        for dialect in DIALECTS {
            assert!(scan_local(dialect, "").is_empty(), "{dialect:?}");
            assert!(
                scan_local(dialect, &render_local_stub(dialect)).is_empty(),
                "{dialect:?}"
            );
        }
    }

    #[test]
    fn scan_local_flags_reserved_directives_with_their_hint() {
        let cases: &[(OverrideDialect, &str, usize, &str)] = &[
            (
                OverrideDialect::MyCnf,
                "bind_address = 0.0.0.0\n",
                1,
                "bind_address",
            ),
            (
                OverrideDialect::MyCnf,
                "# note\nskip-name-resolve\n",
                2,
                "skip-name-resolve",
            ),
            (OverrideDialect::PostgresConf, "\nport = 6000\n", 2, "port"),
            (
                OverrideDialect::RedisConf,
                "maxmemory 1gb\ndaemonize yes\n",
                2,
                "daemonize",
            ),
        ];
        for (dialect, content, line, key) in cases {
            let issues = scan_local(*dialect, content);
            assert_eq!(issues.len(), 1, "{dialect:?} {content:?}");
            let issue = issues.first().unwrap();
            assert_eq!(issue.line, *line);
            assert_eq!(issue.key.as_deref(), Some(*key));
            assert!(
                matches!(issue.problem, LocalOverrideProblem::Reserved { hint } if !hint.is_empty()),
                "{dialect:?} {content:?}"
            );
        }
    }

    #[test]
    fn scan_local_flags_lines_that_are_not_directives() {
        let cases: &[(OverrideDialect, &str, usize)] = &[
            (OverrideDialect::MyCnf, "this is not valid\n", 1),
            (OverrideDialect::MyCnf, "1bad = 2\n", 1),
            (OverrideDialect::PostgresConf, "just_a_key\n", 1),
            (OverrideDialect::PostgresConf, "[mysqld]\n", 1),
            (OverrideDialect::RedisConf, "maxmemory\n", 1),
            (OverrideDialect::RedisConf, "= 5\n", 1),
        ];
        for (dialect, content, line) in cases {
            let issues = scan_local(*dialect, content);
            assert_eq!(issues.len(), 1, "{dialect:?} {content:?}");
            let issue = issues.first().unwrap();
            assert_eq!(issue.line, *line, "{content:?}");
            assert_eq!(issue.key, None, "{content:?}");
            assert_eq!(
                issue.problem,
                LocalOverrideProblem::Malformed,
                "{content:?}"
            );
        }
    }

    #[test]
    fn error_display_non_empty() {
        for r in [
            OverrideNameErrorReason::Empty,
            OverrideNameErrorReason::TooLong,
            OverrideNameErrorReason::IllegalStart,
            OverrideNameErrorReason::IllegalCharacter,
        ] {
            assert!(!r.to_string().is_empty());
        }
        for r in [
            OverrideValueErrorReason::Empty,
            OverrideValueErrorReason::TooLong,
            OverrideValueErrorReason::IllegalCharacter,
            OverrideValueErrorReason::Quote,
        ] {
            assert!(!r.to_string().is_empty());
        }
        for p in [
            LocalOverrideProblem::Reserved { hint: "hint" },
            LocalOverrideProblem::Malformed,
        ] {
            assert!(!p.to_string().is_empty());
        }
        assert!(!OverrideError::Name {
            reason: OverrideNameErrorReason::Empty
        }
        .to_string()
        .is_empty());
        assert!(!OverrideError::Value {
            reason: OverrideValueErrorReason::Quote
        }
        .to_string()
        .is_empty());
    }
}
