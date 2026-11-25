//! Pure helpers for the ngrok and zrok one-shot commands.
//!
//! Argument vectors for `zrok2 enable`/`zrok2 disable` and the `version`
//! probe both binaries answer, plus the output handling around them: version
//! parsing, secret redaction, and picking the line to show when a command
//! fails. Like [`crate::args`], the caller builds the actual command from the
//! resolved binary path, so this module performs no I/O.

use std::ffi::OsString;

/// Args for `zrok2 enable --headless <token>`.
///
/// zrok v2 takes the account token only as a positional argument. The caller
/// must already have rejected tokens starting with `-`, so it can never be
/// parsed as a flag.
#[must_use]
pub fn zrok_enable_args(token: &str) -> Vec<OsString> {
    vec!["enable".into(), "--headless".into(), token.into()]
}

/// Args for `zrok2 disable`, which reads the account token from the enabled
/// environment itself.
#[must_use]
pub fn zrok_disable_args() -> Vec<OsString> {
    vec!["disable".into()]
}

/// Args for the version probe: both `ngrok version` and `zrok2 version`
/// (zrok2 has no `--version` flag).
#[must_use]
pub fn version_args() -> Vec<OsString> {
    vec!["version".into()]
}

/// Finds the first `N.N.N` version in a `version` command's output, without
/// a leading `v`.
///
/// Each whitespace token is trimmed of characters other than ASCII
/// alphanumerics and `.`, then one leading `v` is dropped; the token matches
/// only when exactly three non-empty digit runs remain, so `1.2.3.4` and
/// `DEV` do not.
#[must_use]
pub fn parse_version_token(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|raw| {
        let trimmed = raw.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '.'));
        let bare = trimmed.strip_prefix('v').unwrap_or(trimmed);
        let parts: Vec<&str> = bare.split('.').collect();
        let numeric = parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
        (parts.len() == 3 && numeric).then(|| bare.to_owned())
    })
}

/// Strips one leading `v`, so a zrok tag (`v2.0.11`) and a probed version
/// (`2.0.11`) display the same.
#[must_use]
pub fn display_version(v: &str) -> &str {
    v.strip_prefix('v').unwrap_or(v)
}

/// Replaces every occurrence of a non-empty `secret` in `text` with `***`.
#[must_use]
pub fn redact(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        text.to_owned()
    } else {
        text.replace(secret, "***")
    }
}

/// The line to show when a one-shot fails: the last non-blank stderr line,
/// redacted of `secret` and with a leading `[ERROR]` label stripped.
///
/// When stderr has nothing usable it falls back to the last stdout line that
/// starts with `[ERROR]` (also redacted), then to `"command failed"`.
#[must_use]
pub fn failure_line(stderr: &str, stdout: &str, secret: &str) -> String {
    let from_stderr = last_line(&redact(stderr, secret), |_| true);
    let from_stdout = || last_line(&redact(stdout, secret), |l| l.starts_with("[ERROR]"));
    from_stderr
        .or_else(from_stdout)
        .unwrap_or_else(|| "command failed".to_owned())
}

fn last_line(text: &str, keep: impl Fn(&str) -> bool) -> Option<String> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty() && keep(l))
        .map(strip_error_label)
        .filter(|l| !l.is_empty())
}

fn strip_error_label(line: &str) -> String {
    line.strip_prefix("[ERROR]:")
        .or_else(|| line.strip_prefix("[ERROR]"))
        .unwrap_or(line)
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[OsString]) -> Vec<&str> {
        args.iter().filter_map(|a| a.to_str()).collect()
    }

    #[test]
    fn argv_builders() {
        assert_eq!(
            strings(&zrok_enable_args("tok")),
            ["enable", "--headless", "tok"]
        );
        assert_eq!(strings(&zrok_disable_args()), ["disable"]);
        assert_eq!(strings(&version_args()), ["version"]);
    }

    #[test]
    fn display_version_cases() {
        for (input, expected) in [
            ("v2.0.11", "2.0.11"),
            ("2.0.11", "2.0.11"),
            ("vv1", "v1"),
            ("", ""),
        ] {
            assert_eq!(display_version(input), expected, "{input:?}");
        }
    }

    #[test]
    fn parse_version_token_cases() {
        let zrok = "\n  _____ __ ___ | | __\n |_  / '__/ _ \\| |/ /\n  / /| | | (_) |   <\n /___|_|  \\___/|_|\\_\\\n\nv2.0.11 [abc123]\n";
        for (input, expected) in [
            ("ngrok version 3.18.4", Some("3.18.4")),
            (zrok, Some("2.0.11")),
            ("DEV", None),
            ("1.2.3.4", None),
            ("version (3.39.13)", Some("3.39.13")),
            ("", None),
            ("1.2", None),
        ] {
            assert_eq!(parse_version_token(input).as_deref(), expected, "{input:?}");
        }
    }

    #[test]
    fn redact_cases() {
        for (text, secret, expected) in [
            ("tok and tok", "tok", "*** and ***"),
            ("nothing here", "tok", "nothing here"),
            ("keep tok", "", "keep tok"),
        ] {
            assert_eq!(redact(text, secret), expected, "{text:?}");
        }
    }

    #[test]
    fn failure_line_cases() {
        for (stderr, stdout, secret, expected) in [
            (
                "first\n[ERROR]: unable to enable tok123\n\n",
                "",
                "tok123",
                "unable to enable ***",
            ),
            ("[ERROR] disable first\n", "", "", "disable first"),
            ("noise\nlast line  \n  \n", "", "", "last line"),
            (
                "",
                "banner\n[ERROR]: bad token tok123\ntrailer\n",
                "tok123",
                "bad token ***",
            ),
            ("  \n", "no error label\n", "", "command failed"),
            ("", "", "", "command failed"),
        ] {
            assert_eq!(
                failure_line(stderr, stdout, secret),
                expected,
                "{stderr:?} / {stdout:?}"
            );
        }
    }
}
