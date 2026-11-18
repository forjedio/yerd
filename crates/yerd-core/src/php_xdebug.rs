//! Pure model for the bundled, per-version Xdebug toggle.
//!
//! Yerd ships a prebuilt `xdebug.so` per supported PHP minor and loads it into a
//! version's FPM pool only while that version's toggle is on. This module owns
//! the typed settings (modes, start), their validation and request-merge rules,
//! the platform availability matrix, the managed ini directives and the helpers
//! that keep a hand-registered Xdebug from being loaded twice.
//!
//! `xdebug.mode` is a startup-only setting: it is ignored in an FPM pool's
//! `php_value[…]`, so the managed directives are passed on the FPM command line
//! as `-d key=value` by `yerd-php`. That is why [`managed_directives`] returns
//! plain pairs rather than rendered pool lines.
//!
//! This module is pure: no I/O, no clock, no environment reads.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::php::PhpVersion;

/// The host Xdebug connects to. Yerd is a local environment, so the IDE is on
/// loopback.
pub const CLIENT_HOST: &str = "127.0.0.1";

/// The Xdebug 3 default IDE port.
pub const CLIENT_PORT: u16 = 9003;

/// Keys Yerd owns while managed Xdebug is on. A free-form directive with one of
/// these names is dropped from the pool so it can't fight the managed value.
pub const MANAGED_KEYS: [&str; 3] = ["xdebug.mode", "xdebug.start_with_request", "opcache.jit"];

/// The lowest PHP minor an `xdebug.so` is published for.
const FIRST_MINOR: PhpVersion = PhpVersion::new(8, 2);

/// The highest PHP minor an `xdebug.so` is published for.
const LAST_MINOR: PhpVersion = PhpVersion::new(8, 5);

/// `(os, arch)` pairs an `xdebug.so` is published for, spelled as
/// `std::env::consts` spells them.
const PLATFORMS: [(&str, &str); 3] = [
    ("linux", "x86_64"),
    ("linux", "aarch64"),
    ("macos", "aarch64"),
];

/// One Xdebug mode (`xdebug.mode` list member). `off` is deliberately absent:
/// turning Xdebug off is the toggle's job, not a mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XdebugMode {
    /// Step debugging.
    Debug,
    /// Development helpers (improved `var_dump`, stack traces).
    Develop,
    /// Code coverage.
    Coverage,
    /// Profiler (writes cachegrind files to the output dir).
    Profile,
    /// Function trace (writes trace files to the output dir).
    Trace,
    /// Garbage-collection statistics (writes report files to the output dir).
    Gcstats,
}

impl XdebugMode {
    /// Every mode, in declaration (and normalised) order.
    pub const ALL: [Self; 6] = [
        Self::Debug,
        Self::Develop,
        Self::Coverage,
        Self::Profile,
        Self::Trace,
        Self::Gcstats,
    ];

    /// The `xdebug.mode` spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Develop => "develop",
            Self::Coverage => "coverage",
            Self::Profile => "profile",
            Self::Trace => "trace",
            Self::Gcstats => "gcstats",
        }
    }

    /// True for the modes that write files to `xdebug.output_dir`.
    #[must_use]
    pub const fn writes_output(self) -> bool {
        matches!(self, Self::Profile | Self::Trace | Self::Gcstats)
    }
}

impl fmt::Display for XdebugMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for XdebugMode {
    type Err = XdebugError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let wanted = s.trim();
        if wanted.eq_ignore_ascii_case("off") {
            return Err(XdebugError::ModeOff);
        }
        Self::ALL
            .into_iter()
            .find(|m| m.as_str().eq_ignore_ascii_case(wanted))
            .ok_or_else(|| XdebugError::UnknownMode(wanted.to_owned()))
    }
}

/// When Xdebug starts a session (`xdebug.start_with_request`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XdebugStart {
    /// On every request (`yes`).
    #[default]
    Always,
    /// Only when a trigger (cookie, query or env) is present (`trigger`).
    Trigger,
}

impl XdebugStart {
    /// The config and wire spelling (`always` / `trigger`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Trigger => "trigger",
        }
    }

    /// The `xdebug.start_with_request` value.
    #[must_use]
    pub const fn ini_value(self) -> &'static str {
        match self {
            Self::Always => "yes",
            Self::Trigger => "trigger",
        }
    }
}

impl fmt::Display for XdebugStart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for XdebugStart {
    type Err = XdebugError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let wanted = s.trim();
        if wanted.eq_ignore_ascii_case("always") {
            Ok(Self::Always)
        } else if wanted.eq_ignore_ascii_case("trigger") {
            Ok(Self::Trigger)
        } else {
            Err(XdebugError::UnknownStart(wanted.to_owned()))
        }
    }
}

/// One PHP version's Xdebug settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XdebugSettings {
    /// Whether the version's FPM pool loads Xdebug.
    pub enabled: bool,
    /// The `xdebug.mode` list; never empty once validated.
    pub modes: Vec<XdebugMode>,
    /// When a session starts.
    pub start: XdebugStart,
}

impl Default for XdebugSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            modes: vec![XdebugMode::Debug],
            start: XdebugStart::Always,
        }
    }
}

impl XdebugSettings {
    /// Sort and dedup `modes` in declaration order so equality and diffs are
    /// stable whatever order the user typed them in.
    pub fn normalize(&mut self) {
        self.modes.sort_unstable();
        self.modes.dedup();
    }

    /// True when profile, trace or gcstats is selected, so `xdebug.output_dir`
    /// matters.
    #[must_use]
    pub fn writes_output(&self) -> bool {
        self.modes.iter().any(|m| m.writes_output())
    }

    /// The comma-joined `xdebug.mode` value.
    #[must_use]
    pub fn mode_value(&self) -> String {
        self.modes
            .iter()
            .map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Validate settings: the mode list must not be empty.
///
/// # Errors
/// [`XdebugError::EmptyModes`] when no mode is selected.
pub fn validate(settings: &XdebugSettings) -> Result<(), XdebugError> {
    if settings.modes.is_empty() {
        return Err(XdebugError::EmptyModes);
    }
    Ok(())
}

/// Apply a set-Xdebug request to the current settings. Absent
/// `enabled`/`modes`/`start` keep the current value, or the default when there
/// is none. The result is normalised and validated.
///
/// Xdebug starts GC statistics only when `xdebug.start_with_request=yes` (its
/// trigger check never covers `gcstats`), so a request that would leave
/// `gcstats` with the trigger start enabled is refused. Disabling is always
/// allowed, even with that pair saved. This is a request-time check only: a
/// hand-edited config with that pair still loads, it just collects no GC
/// statistics automatically.
///
/// # Errors
/// [`XdebugError::EmptyModes`] when the request supplies an empty mode list;
/// [`XdebugError::GcstatsNeedsAlways`] for an enabled `gcstats` with the
/// trigger start.
pub fn merge_request(
    current: Option<&XdebugSettings>,
    enabled: Option<bool>,
    modes: Option<Vec<XdebugMode>>,
    start: Option<XdebugStart>,
) -> Result<XdebugSettings, XdebugError> {
    let base = current.cloned().unwrap_or_default();
    let mut merged = XdebugSettings {
        enabled: enabled.unwrap_or(base.enabled),
        modes: modes.unwrap_or(base.modes),
        start: start.unwrap_or(base.start),
    };
    merged.normalize();
    validate(&merged)?;
    if merged.enabled
        && merged.start == XdebugStart::Trigger
        && merged.modes.contains(&XdebugMode::Gcstats)
    {
        return Err(XdebugError::GcstatsNeedsAlways);
    }
    Ok(merged)
}

/// Whether an `xdebug.so` is published for `v` on `os`/`arch` (spelled as
/// `std::env::consts` spells them). Legacy minors are never available.
#[must_use]
pub fn is_available(v: PhpVersion, os: &str, arch: &str) -> bool {
    !v.is_legacy()
        && (FIRST_MINOR..=LAST_MINOR).contains(&v)
        && PLATFORMS.iter().any(|(o, a)| *o == os && *a == arch)
}

/// True if `name` is one of the [`MANAGED_KEYS`].
#[must_use]
pub fn is_managed_key(name: &str) -> bool {
    MANAGED_KEYS.contains(&name)
}

/// The directives Yerd sets while Xdebug is on, in emission order.
///
/// `opcache.jit=disable` because Xdebug and the JIT are incompatible, and
/// unlike `off` it can't be re-enabled at runtime. `xdebug.log_level=0`
/// silences the "Could not connect to debugging client" error printed on every
/// request when no IDE is listening. `xdebug.output_dir` is only emitted when
/// profile, trace or gcstats is selected.
#[must_use]
pub fn managed_directives(
    settings: &XdebugSettings,
    output_dir: &str,
) -> Vec<(&'static str, String)> {
    let mut out = vec![
        ("xdebug.mode", settings.mode_value()),
        (
            "xdebug.start_with_request",
            settings.start.ini_value().to_owned(),
        ),
        ("xdebug.client_host", CLIENT_HOST.to_owned()),
        ("xdebug.client_port", CLIENT_PORT.to_string()),
        ("xdebug.log_level", "0".to_owned()),
        ("opcache.jit", "disable".to_owned()),
    ];
    if settings.writes_output() {
        out.push(("xdebug.output_dir", output_dir.to_owned()));
    }
    out
}

/// True if a hand-registered extension is an Xdebug: its name is `xdebug` or its
/// path's basename starts with `xdebug` (`xdebug.so`, `xdebug-3.4.so`,
/// `xdebug_custom.so`), compared case-insensitively.
///
/// This is a heuristic: an Xdebug renamed to something unrelated isn't
/// recognised, and an unrelated `.so` whose name starts with `xdebug` is.
#[must_use]
pub fn is_xdebug_extension(name: &str, path: &str) -> bool {
    if name.trim().eq_ignore_ascii_case("xdebug") {
        return true;
    }
    Path::new(path.trim())
        .file_name()
        .and_then(|f| f.to_str())
        .and_then(|f| f.get(..6))
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("xdebug"))
}

/// True if any `extension = …` or `zend_extension = …` line in `ini` loads an
/// Xdebug (per [`is_xdebug_extension`]). See [`ini_load_paths`] for how each
/// value is read.
#[must_use]
pub fn ini_loads_xdebug(ini: &str) -> bool {
    ini_load_paths(ini)
        .iter()
        .any(|path| is_xdebug_extension("", path))
}

/// The paths of every `extension = …` and `zend_extension = …` line in `ini`,
/// in file order. A trailing `;` comment outside quotes is dropped and quotes
/// around the value are removed. Inside double quotes `\"`, `\\` and `\$`
/// decode to the escaped character, as PHP's ini scanner does; any other
/// backslash is kept.
#[must_use]
pub fn ini_load_paths(ini: &str) -> Vec<String> {
    ini.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            (key.eq_ignore_ascii_case("extension") || key.eq_ignore_ascii_case("zend_extension"))
                .then(|| unquote_ini_value(strip_ini_comment(value).trim()))
        })
        .collect()
}

/// `value` up to the first `;` that is outside single or double quotes. A
/// backslash inside double quotes escapes the next character, so `\"` doesn't
/// close the quote.
fn strip_ini_comment(value: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            (None, ';') => return value.get(..i).unwrap_or(value),
            (None, '"' | '\'') => quote = Some(c),
            (Some('"'), '\\') => escaped = true,
            (Some(q), _) if q == c => quote = None,
            _ => {}
        }
    }
    value
}

/// The text of an ini value with its surrounding quotes removed and double-quote
/// escapes decoded (see [`ini_load_paths`]). An unquoted value is returned as is.
fn unquote_ini_value(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some('"') => {
            let mut out = String::with_capacity(value.len());
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some(n @ ('"' | '\\' | '$')) => out.push(n),
                        Some(n) => {
                            out.push('\\');
                            out.push(n);
                        }
                        None => out.push('\\'),
                    },
                    _ => out.push(c),
                }
            }
            out
        }
        Some('\'') => chars.take_while(|c| *c != '\'').collect(),
        _ => value.to_owned(),
    }
}

/// `path` as a double-quoted ini string: `"`, `\` and `$` are backslash-escaped
/// so the value survives PHP's ini scanner (which expands `${…}` inside double
/// quotes). `None` for a non-UTF-8 path or one with a control character, which
/// would break the line.
fn quote_ini_path(path: &Path) -> Option<String> {
    let s = path.to_str()?;
    if s.chars().any(char::is_control) {
        return None;
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if matches!(c, '"' | '\\' | '$') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    Some(out)
}

/// Render the debug-shim ini: `zend_extension = "<so>"` plus the managed
/// `xdebug.*` directives for the default settings, then `base` (the CLI ini) so
/// the user's own per-version `xdebug.*` directives win, then
/// `opcache.jit = disable` last so a per-version `opcache.jit` in `base` can't
/// re-enable the JIT under Xdebug. The path is quoted (see [`quote_ini_path`])
/// so characters such as `(`, `;` or `#` in it are safe. Returns `None` for a
/// non-UTF-8 path or one with a control character.
#[must_use]
pub fn render_debug_ini(base: &str, so: &Path) -> Option<String> {
    let path = quote_ini_path(so)?;
    let settings = XdebugSettings {
        enabled: true,
        ..XdebugSettings::default()
    };
    let (jit, xdebug): (Vec<_>, Vec<_>) = managed_directives(&settings, "")
        .into_iter()
        .partition(|(key, _)| *key == "opcache.jit");
    let mut out = format!("zend_extension = {path}\n");
    push_lines(&mut out, &xdebug);
    out.push_str(base);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    push_lines(&mut out, &jit);
    Some(out)
}

fn push_lines(out: &mut String, lines: &[(&str, String)]) {
    for (key, value) in lines {
        out.push_str(key);
        out.push_str(" = ");
        out.push_str(value);
        out.push('\n');
    }
}

/// Failure to parse or validate Xdebug settings.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum XdebugError {
    /// No mode was selected.
    #[error("select at least one Xdebug mode")]
    EmptyModes,
    /// `off` was given as a mode.
    #[error("`off` is not a mode; use `yerd xdebug off` to turn Xdebug off")]
    ModeOff,
    /// An unrecognised mode name.
    #[error(
        "unknown Xdebug mode '{0}' (expected debug, develop, coverage, profile, trace or gcstats)"
    )]
    UnknownMode(String),
    /// An unrecognised start value.
    #[error("unknown Xdebug start '{0}' (expected always or trigger)")]
    UnknownStart(String),
    /// `gcstats` was combined with the trigger start, which Xdebug never uses
    /// to start GC statistics.
    #[error("Xdebug's gcstats mode only runs with the `always` start; it ignores triggers")]
    GcstatsNeedsAlways,
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse_and_round_trip() {
        for m in XdebugMode::ALL {
            assert_eq!(m.as_str().parse::<XdebugMode>().unwrap(), m);
            assert_eq!(m.to_string(), m.as_str());
        }
        assert_eq!(" Debug ".parse::<XdebugMode>().unwrap(), XdebugMode::Debug);
    }

    #[test]
    fn mode_parse_errors() {
        let cases = [
            ("off", XdebugError::ModeOff),
            ("OFF", XdebugError::ModeOff),
            ("", XdebugError::UnknownMode(String::new())),
            ("debugger", XdebugError::UnknownMode("debugger".into())),
        ];
        for (input, want) in cases {
            assert_eq!(input.parse::<XdebugMode>().unwrap_err(), want, "{input}");
        }
    }

    #[test]
    fn start_parse_and_ini_values() {
        let cases = [
            ("always", XdebugStart::Always, "yes"),
            ("Trigger", XdebugStart::Trigger, "trigger"),
        ];
        for (input, want, ini) in cases {
            let got = input.parse::<XdebugStart>().unwrap();
            assert_eq!(got, want);
            assert_eq!(got.ini_value(), ini);
        }
        assert_eq!(
            "yes".parse::<XdebugStart>().unwrap_err(),
            XdebugError::UnknownStart("yes".into())
        );
    }

    #[test]
    fn default_is_disabled_debug_always() {
        let d = XdebugSettings::default();
        assert!(!d.enabled);
        assert_eq!(d.modes, vec![XdebugMode::Debug]);
        assert_eq!(d.start, XdebugStart::Always);
    }

    #[test]
    fn normalize_sorts_and_dedups() {
        let mut s = XdebugSettings {
            enabled: true,
            modes: vec![
                XdebugMode::Trace,
                XdebugMode::Debug,
                XdebugMode::Trace,
                XdebugMode::Develop,
            ],
            start: XdebugStart::Always,
        };
        s.normalize();
        assert_eq!(
            s.modes,
            vec![XdebugMode::Debug, XdebugMode::Develop, XdebugMode::Trace]
        );
        assert_eq!(s.mode_value(), "debug,develop,trace");
    }

    #[test]
    fn validate_rejects_empty_modes() {
        let mut s = XdebugSettings::default();
        assert!(validate(&s).is_ok());
        s.modes.clear();
        assert_eq!(validate(&s), Err(XdebugError::EmptyModes));
    }

    type MergeCase<'a> = (
        Option<&'a XdebugSettings>,
        Option<bool>,
        Option<Vec<XdebugMode>>,
        Option<XdebugStart>,
        XdebugSettings,
    );

    #[test]
    fn merge_request_cases() {
        let current = XdebugSettings {
            enabled: false,
            modes: vec![XdebugMode::Profile],
            start: XdebugStart::Trigger,
        };
        let on = XdebugSettings {
            enabled: true,
            ..XdebugSettings::default()
        };
        let cases: [MergeCase<'_>; 6] = [
            (
                None,
                Some(true),
                None,
                None,
                XdebugSettings {
                    enabled: true,
                    ..XdebugSettings::default()
                },
            ),
            (
                Some(&current),
                Some(true),
                None,
                None,
                XdebugSettings {
                    enabled: true,
                    modes: vec![XdebugMode::Profile],
                    start: XdebugStart::Trigger,
                },
            ),
            (
                Some(&current),
                Some(false),
                Some(vec![XdebugMode::Trace, XdebugMode::Debug]),
                None,
                XdebugSettings {
                    enabled: false,
                    modes: vec![XdebugMode::Debug, XdebugMode::Trace],
                    start: XdebugStart::Trigger,
                },
            ),
            (
                Some(&current),
                Some(true),
                None,
                Some(XdebugStart::Always),
                XdebugSettings {
                    enabled: true,
                    modes: vec![XdebugMode::Profile],
                    start: XdebugStart::Always,
                },
            ),
            (
                Some(&on),
                None,
                Some(vec![XdebugMode::Trace]),
                None,
                XdebugSettings {
                    enabled: true,
                    modes: vec![XdebugMode::Trace],
                    start: XdebugStart::Always,
                },
            ),
            (
                None,
                None,
                None,
                Some(XdebugStart::Trigger),
                XdebugSettings {
                    start: XdebugStart::Trigger,
                    ..XdebugSettings::default()
                },
            ),
        ];
        for (cur, enabled, modes, start, want) in cases {
            assert_eq!(merge_request(cur, enabled, modes, start).unwrap(), want);
        }
        assert_eq!(
            merge_request(Some(&current), Some(true), Some(Vec::new()), None),
            Err(XdebugError::EmptyModes)
        );
    }

    #[test]
    fn gcstats_with_trigger_is_refused() {
        let trigger = XdebugSettings {
            start: XdebugStart::Trigger,
            ..XdebugSettings::default()
        };
        let always_gc = XdebugSettings {
            modes: vec![XdebugMode::Gcstats],
            ..XdebugSettings::default()
        };
        let cases = [
            (Some(&trigger), Some(vec![XdebugMode::Gcstats]), None),
            (
                Some(&trigger),
                Some(vec![XdebugMode::Debug, XdebugMode::Gcstats]),
                None,
            ),
            (Some(&always_gc), None, Some(XdebugStart::Trigger)),
            (
                None,
                Some(vec![XdebugMode::Gcstats]),
                Some(XdebugStart::Trigger),
            ),
        ];
        for (cur, modes, start) in cases {
            assert_eq!(
                merge_request(cur, Some(true), modes.clone(), start),
                Err(XdebugError::GcstatsNeedsAlways),
                "{modes:?} {start:?}"
            );
        }
        assert!(merge_request(Some(&always_gc), Some(true), None, None).is_ok());
    }

    #[test]
    fn a_saved_gcstats_trigger_pair_can_always_be_disabled() {
        let saved = XdebugSettings {
            enabled: true,
            modes: vec![XdebugMode::Gcstats],
            start: XdebugStart::Trigger,
        };
        assert_eq!(
            merge_request(Some(&saved), Some(false), None, None),
            Ok(XdebugSettings {
                enabled: false,
                ..saved.clone()
            })
        );
        let off = XdebugSettings {
            enabled: false,
            ..saved.clone()
        };
        for enabled in [None, Some(false)] {
            assert_eq!(
                merge_request(Some(&off), enabled, None, None),
                Ok(off.clone()),
                "{enabled:?}"
            );
        }
        assert_eq!(
            merge_request(Some(&off), Some(true), None, None),
            Err(XdebugError::GcstatsNeedsAlways)
        );
        assert_eq!(
            merge_request(Some(&saved), None, None, None),
            Err(XdebugError::GcstatsNeedsAlways)
        );
    }

    #[test]
    fn ini_load_paths_decode_quotes_and_escapes() {
        let ini = "memory_limit = 1G\n\
                   extension = /x/redis.so ; c\n\
                   zend_extension = \"/opt/a\\\";b/x.so\"\n\
                   zend_extension = \"/opt/\\\\\\$d\\n/x.so\"\n\
                   EXTENSION = '/opt/s\\q.so' ; c\n\
                   ; extension = /x/off.so\n";
        assert_eq!(
            ini_load_paths(ini),
            vec![
                "/x/redis.so".to_owned(),
                "/opt/a\";b/x.so".to_owned(),
                "/opt/\\$d\\n/x.so".to_owned(),
                "/opt/s\\q.so".to_owned(),
            ]
        );
        assert!(ini_load_paths("").is_empty());
    }

    #[test]
    fn availability_matrix() {
        let cases = [
            ((8, 2), "linux", "x86_64", true),
            ((8, 3), "linux", "aarch64", true),
            ((8, 4), "macos", "aarch64", true),
            ((8, 5), "macos", "aarch64", true),
            ((8, 1), "macos", "aarch64", false),
            ((7, 4), "linux", "x86_64", false),
            ((8, 6), "linux", "x86_64", false),
            ((8, 4), "macos", "x86_64", false),
            ((8, 4), "windows", "x86_64", false),
        ];
        for ((major, minor), os, arch, want) in cases {
            assert_eq!(
                is_available(PhpVersion::new(major, minor), os, arch),
                want,
                "{major}.{minor} {os}/{arch}"
            );
        }
    }

    #[test]
    fn managed_keys_match() {
        for (key, want) in [
            ("xdebug.mode", true),
            ("xdebug.start_with_request", true),
            ("opcache.jit", true),
            ("xdebug.idekey", false),
            ("xdebug.client_port", false),
            ("opcache.jit_buffer_size", false),
        ] {
            assert_eq!(is_managed_key(key), want, "{key}");
        }
    }

    #[test]
    fn managed_directives_defaults() {
        let s = XdebugSettings {
            enabled: true,
            ..XdebugSettings::default()
        };
        let got = managed_directives(&s, "/data/xdebug");
        let want: Vec<(&str, String)> = vec![
            ("xdebug.mode", "debug".into()),
            ("xdebug.start_with_request", "yes".into()),
            ("xdebug.client_host", "127.0.0.1".into()),
            ("xdebug.client_port", "9003".into()),
            ("xdebug.log_level", "0".into()),
            ("opcache.jit", "disable".into()),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn output_dir_only_for_output_writing_modes() {
        let cases = [
            (vec![XdebugMode::Debug], false),
            (vec![XdebugMode::Develop, XdebugMode::Coverage], false),
            (vec![XdebugMode::Gcstats], true),
            (vec![XdebugMode::Profile], true),
            (vec![XdebugMode::Debug, XdebugMode::Trace], true),
        ];
        for (modes, want) in cases {
            let s = XdebugSettings {
                enabled: true,
                modes,
                start: XdebugStart::Trigger,
            };
            let got = managed_directives(&s, "/d/xdebug");
            let has = got
                .iter()
                .any(|(k, v)| *k == "xdebug.output_dir" && v == "/d/xdebug");
            assert_eq!(has, want, "{:?}", s.modes);
            assert_eq!(got[1].1, "trigger");
        }
    }

    #[test]
    fn xdebug_extension_matcher() {
        let cases = [
            ("xdebug", "/opt/whatever.so", true),
            ("XDebug", "/opt/whatever.so", true),
            ("dbg", "/opt/ext/xdebug.so", true),
            ("dbg", "/opt/ext/XDEBUG.SO", true),
            ("dbg", "xdebug.so", true),
            ("dbg", "/opt/ext/xdebug-3.4.so", true),
            ("dbg", "/opt/ext/xdebug_custom.so", true),
            ("dbg", "/opt/ext/XDebug-3.so", true),
            ("pcov", "/opt/ext/pcov.so", false),
            ("dbg", "/opt/xdebug/pcov.so", false),
            ("dbg", "/opt/ext/xdbg.so", false),
            ("xdebugger", "/opt/ext/x.so", false),
        ];
        for (name, path, want) in cases {
            assert_eq!(is_xdebug_extension(name, path), want, "{name} {path}");
        }
    }

    #[test]
    fn ini_loads_xdebug_cases() {
        let cases = [
            ("zend_extension = \"/x/xdebug.so\"\n", true),
            ("zend_extension=/x/xdebug.so\n", true),
            ("extension = '/x/xdebug.so'\n", true),
            ("memory_limit = 1G\nzend_extension = /x/xdebug.so\n", true),
            ("extension = /x/pcov.so\n", false),
            ("xdebug.mode = debug\n", false),
            ("; zend_extension = /x/xdebug.so\n", false),
            ("zend_extension = /x/xdebug.so ; loaded by hand\n", true),
            ("zend_extension = /x/xdebug.so;comment\n", true),
            ("zend_extension = \"/x/a;b/xdebug.so\" ; c\n", true),
            ("zend_extension = \"/opt/a\\\";b/xdebug.so\"\n", true),
            (
                "zend_extension = \"/opt/a\\\";b/pcov.so\" ; xdebug.so\n",
                false,
            ),
            ("extension = /x/pcov.so ; xdebug.so\n", false),
            ("", false),
        ];
        for (ini, want) in cases {
            assert_eq!(ini_loads_xdebug(ini), want, "{ini:?}");
        }
    }

    #[test]
    fn render_debug_ini_puts_xdebug_before_base_and_jit_after() {
        let cases = [
            (
                "memory_limit = 1G\nxdebug.mode = coverage",
                "memory_limit = 1G\nxdebug.mode = coverage\n",
            ),
            ("opcache.jit = tracing\n", "opcache.jit = tracing\n"),
            ("", ""),
        ];
        for (base, body) in cases {
            let got = render_debug_ini(base, Path::new("/d/xdebug.so")).unwrap();
            let want = format!(
                "zend_extension = \"/d/xdebug.so\"\n\
                 xdebug.mode = debug\n\
                 xdebug.start_with_request = yes\n\
                 xdebug.client_host = 127.0.0.1\n\
                 xdebug.client_port = 9003\n\
                 xdebug.log_level = 0\n\
                 {body}\
                 opcache.jit = disable\n"
            );
            assert_eq!(got, want, "{base:?}");
            assert!(got.ends_with("opcache.jit = disable\n"), "{base:?}");
        }
    }

    #[test]
    fn render_debug_ini_quotes_and_escapes_the_path() {
        let cases = [
            (
                "/Users/me (work)/xdebug.so",
                "zend_extension = \"/Users/me (work)/xdebug.so\"\n",
            ),
            (
                "/d/x;y#z/xdebug.so",
                "zend_extension = \"/d/x;y#z/xdebug.so\"\n",
            ),
            (
                "/d/a\"b\\c$d/xdebug.so",
                "zend_extension = \"/d/a\\\"b\\\\c\\$d/xdebug.so\"\n",
            ),
        ];
        for (path, first) in cases {
            let got = render_debug_ini("", Path::new(path)).unwrap();
            assert!(got.starts_with(first), "{path}: {got}");
        }
    }

    #[test]
    fn render_debug_ini_rejects_control_characters() {
        assert!(render_debug_ini("", Path::new("/d/x\ny/xdebug.so")).is_none());
        assert!(render_debug_ini("", Path::new("/d/x\ty/xdebug.so")).is_none());
    }
}
