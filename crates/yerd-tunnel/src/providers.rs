//! Pure argv, config and output handling for ngrok and zrok.
//!
//! Argument vectors for the one-shot commands (`zrok2 enable`/`disable`, the
//! `version` probe) and for the supervised shares (`ngrok http`, `zrok2 share
//! public`), the ngrok agent config Yerd runs with, and the output handling
//! around them: version parsing, secret redaction, the public URL each share
//! prints, and the line to show when a command or share fails. Like
//! [`crate::args`], the caller builds the actual command from the resolved
//! binary path, so this module performs no I/O.

use std::ffi::OsString;
use std::path::Path;

/// The env var ngrok reads its authtoken from. Yerd passes the token only
/// this way, never on the command line.
pub const NGROK_AUTHTOKEN_ENV: &str = "NGROK_AUTHTOKEN";

/// The ngrok-owned domains a share without a static domain is served on.
pub const NGROK_DOMAINS: [&str; 5] = [
    "ngrok-free.app",
    "ngrok-free.dev",
    "ngrok.app",
    "ngrok.dev",
    "ngrok.io",
];

/// The text zrok's headless share prints just before its public URLs.
const ZROK_BANNER: &str = "access your zrok share at the following endpoints";

/// Args for `ngrok http <origin> --config <path> [--url https://<domain>]`.
///
/// The explicit `--config` replaces ngrok's default config file, so the
/// user's own `ngrok.yml` is never read. The authtoken is never part of the
/// argv: it goes in [`NGROK_AUTHTOKEN_ENV`].
#[must_use]
pub fn ngrok_share_args(
    origin: &str,
    config_path: &Path,
    static_domain: Option<&str>,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "http".into(),
        origin.into(),
        "--config".into(),
        config_path.as_os_str().to_owned(),
    ];
    if let Some(domain) = static_domain {
        args.push("--url".into());
        args.push(format!("https://{domain}").into());
    }
    args
}

/// The ngrok agent config every Yerd share runs with: JSON log lines on
/// stdout, no local web UI or API, no update check and no console UI. It holds
/// no secret, so every share can use the same file.
#[must_use]
pub fn render_ngrok_config() -> String {
    "version: 3\n\
     agent:\n\
     \x20 web_addr: false\n\
     \x20 log: stdout\n\
     \x20 log_format: json\n\
     \x20 log_level: info\n\
     \x20 update_check: false\n\
     \x20 console_ui: false\n"
        .to_owned()
}

/// Args for `zrok2 share public <origin> --headless --backend-mode proxy
/// --force-local`, an ephemeral public share in the enabled environment.
#[must_use]
pub fn zrok_share_args(origin: &str) -> Vec<OsString> {
    vec![
        "share".into(),
        "public".into(),
        origin.into(),
        "--headless".into(),
        "--backend-mode".into(),
        "proxy".into(),
        "--force-local".into(),
    ]
}

/// The static domain in an argv built by [`ngrok_share_args`]: the host after
/// `--url https://`, or `None` for a random-URL share.
#[must_use]
pub fn ngrok_static_domain(args: &[OsString]) -> Option<String> {
    args.iter()
        .zip(args.iter().skip(1))
        .find(|(flag, _)| flag.as_os_str() == "--url")
        .and_then(|(_, value)| value.to_str()?.strip_prefix("https://").map(str::to_owned))
}

/// The public URL an ngrok share reports in its log, as `https://<host>`.
///
/// Each line's candidate is the `url` field of a JSON object, else the value
/// of a logfmt `url=` pair. Only an `https` URL whose host is `static_domain`
/// or one of [`NGROK_DOMAINS`] (or a subdomain of one) is accepted, so the
/// upstream `addr` or an unrelated URL is never taken for the share.
#[must_use]
pub fn parse_ngrok_url(log: &str, static_domain: Option<&str>) -> Option<String> {
    let wanted = static_domain.map(str::to_ascii_lowercase);
    log.lines()
        .filter_map(ngrok_url_candidate)
        .filter_map(|candidate| https_host(&candidate))
        .find(|host| wanted.as_deref() == Some(host.as_str()) || is_ngrok_host(host))
        .map(|host| format!("https://{host}"))
}

fn ngrok_url_candidate(line: &str) -> Option<String> {
    match json_object(line) {
        Some(obj) => obj.get("url")?.as_str().map(str::to_owned),
        None => logfmt_value(line, "url"),
    }
}

fn json_object(line: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    match serde_json::from_str::<serde_json::Value>(line.trim()).ok()? {
        serde_json::Value::Object(map) => Some(map),
        _ => None,
    }
}

/// The value of a logfmt `key=value` or `key="value"` pair whose key starts
/// the line or follows whitespace.
fn logfmt_value(line: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    line.match_indices(&needle).find_map(|(at, _)| {
        let before = line.get(..at)?;
        if !(before.is_empty() || before.ends_with(char::is_whitespace)) {
            return None;
        }
        let rest = line.get(at + needle.len()..)?;
        let value = match rest.strip_prefix('"') {
            Some(quoted) => quoted.split('"').next()?,
            None => rest.split(char::is_whitespace).next()?,
        };
        (!value.is_empty()).then(|| value.to_owned())
    })
}

/// The lowercased host of an `https://host[:port][/path]` URL.
fn https_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.split(':').next()?.to_ascii_lowercase();
    let valid = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
    valid.then_some(host)
}

fn is_ngrok_host(host: &str) -> bool {
    NGROK_DOMAINS.iter().any(|domain| {
        host == *domain
            || host
                .strip_suffix(domain)
                .is_some_and(|label| label.ends_with('.') && label.len() > 1)
    })
}

/// The first public URL zrok's headless share prints after its "access your
/// zrok share at the following endpoints" banner, as `https://<host>`. `None`
/// until the banner and a URL have both appeared.
#[must_use]
pub fn parse_zrok_url(log: &str) -> Option<String> {
    let (_, after) = log.split_once(ZROK_BANNER)?;
    after
        .split_whitespace()
        .map(|token| token.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ')' | '(')))
        .filter(|token| token.starts_with("https://"))
        .find_map(https_host)
        .map(|host| format!("https://{host}"))
}

/// The message of the last error ngrok logged.
///
/// The last JSON record with `lvl` `eror` or `crit` wins (its `err`, else its
/// `msg`). Without one, the last plain-text line starting `ERROR:` is used
/// (the text after the prefix, or the next non-blank line when the prefix
/// stands alone), with an `ERR_NGROK_<digits>` code from the log appended when
/// the text does not already carry it.
#[must_use]
pub fn last_ngrok_error(log: &str) -> Option<String> {
    log.lines()
        .rev()
        .find_map(ngrok_json_error)
        .or_else(|| ngrok_plain_error(log))
}

fn ngrok_json_error(line: &str) -> Option<String> {
    let obj = json_object(line)?;
    let level = obj.get("lvl")?.as_str()?;
    if level != "eror" && level != "crit" {
        return None;
    }
    ["err", "msg"]
        .iter()
        .filter_map(|key| obj.get(*key)?.as_str())
        .map(str::trim)
        .find(|text| !text.is_empty())
        .map(str::to_owned)
}

fn ngrok_plain_error(log: &str) -> Option<String> {
    let lines: Vec<&str> = log.lines().map(str::trim).collect();
    let (at, line) = lines
        .iter()
        .enumerate()
        .rev()
        .find(|(_, line)| line.starts_with("ERROR:"))?;
    let inline = line.strip_prefix("ERROR:").unwrap_or_default().trim();
    let text = if inline.is_empty() {
        lines.iter().skip(at + 1).find(|l| !l.is_empty())?
    } else {
        inline
    };
    match last_ngrok_code(log) {
        Some(code) if !text.contains(&code) => Some(format!("{text} ({code})")),
        _ => Some(text.to_owned()),
    }
}

fn last_ngrok_code(log: &str) -> Option<String> {
    const PREFIX: &str = "ERR_NGROK_";
    log.match_indices(PREFIX)
        .filter_map(|(at, _)| {
            let digits: String = log
                .get(at + PREFIX.len()..)?
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            (!digits.is_empty()).then(|| format!("{PREFIX}{digits}"))
        })
        .last()
}

/// The message of the last error zrok logged: the text after a `[ERROR]`
/// label or a whitespace-delimited `ERROR` level token, with a leading `:` and
/// spaces trimmed.
#[must_use]
pub fn last_zrok_error(log: &str) -> Option<String> {
    log.lines().rev().find_map(zrok_error_message)
}

fn zrok_error_message(line: &str) -> Option<String> {
    let after = match line.split_once("[ERROR]") {
        Some((_, after)) => after,
        None => after_token(line, "ERROR")?,
    };
    let text = after.trim_start_matches([':', ' ', '\t']).trim_end();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The text after the first whitespace-delimited `token` in `line`.
fn after_token<'a>(line: &'a str, token: &str) -> Option<&'a str> {
    let mut rest = line;
    loop {
        let trimmed = rest.trim_start();
        let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        let (word, after) = trimmed.split_at(end);
        if word.is_empty() {
            return None;
        }
        if word == token {
            return Some(after);
        }
        rest = after;
    }
}

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

/// Oldest ngrok agent that understands `--url` and the v3 agent config Yerd
/// writes.
pub const NGROK_MIN_VERSION: (u32, u32, u32) = (3, 16, 0);

/// Whether an ngrok `version` (`3.39.13`, with or without a leading `v`) is at
/// least [`NGROK_MIN_VERSION`]. An unparseable version counts as supported, so
/// the agent's own error is what the user sees.
#[must_use]
pub fn ngrok_version_supported(version: &str) -> bool {
    let mut parts = display_version(version)
        .split('.')
        .map(|p| p.parse::<u32>().ok());
    match (
        parts.next().flatten(),
        parts.next().flatten(),
        parts.next().flatten(),
    ) {
        (Some(major), Some(minor), Some(patch)) => (major, minor, patch) >= NGROK_MIN_VERSION,
        _ => true,
    }
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

    #[test]
    fn ngrok_version_supported_cases() {
        let cases = [
            ("3.16.0", true),
            ("v3.39.13", true),
            ("4.0.0", true),
            ("3.15.9", false),
            ("2.3.41", false),
            ("dev", true),
        ];
        for (version, want) in cases {
            assert_eq!(ngrok_version_supported(version), want, "{version}");
        }
    }

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
    fn ngrok_share_args_never_carry_the_token() {
        let config = Path::new("/data/tunnel/ngrok.yml");
        let random = ngrok_share_args("http://127.0.0.1:9", config, None);
        assert_eq!(
            strings(&random),
            [
                "http",
                "http://127.0.0.1:9",
                "--config",
                "/data/tunnel/ngrok.yml"
            ]
        );
        let fixed = ngrok_share_args("http://127.0.0.1:9", config, Some("demo.ngrok-free.app"));
        assert_eq!(
            strings(&fixed),
            [
                "http",
                "http://127.0.0.1:9",
                "--config",
                "/data/tunnel/ngrok.yml",
                "--url",
                "https://demo.ngrok-free.app"
            ]
        );
        for args in [&random, &fixed] {
            assert!(args.iter().all(|a| a != "2fXkSampleToken"));
        }
        assert_eq!(
            ngrok_static_domain(&fixed).as_deref(),
            Some("demo.ngrok-free.app")
        );
        assert_eq!(ngrok_static_domain(&random), None);
    }

    #[test]
    fn ngrok_config_text() {
        assert_eq!(
            render_ngrok_config(),
            "version: 3\nagent:\n  web_addr: false\n  log: stdout\n  log_format: json\n  log_level: info\n  update_check: false\n  console_ui: false\n"
        );
    }

    #[test]
    fn zrok_share_argv() {
        assert_eq!(
            strings(&zrok_share_args("http://127.0.0.1:9")),
            [
                "share",
                "public",
                "http://127.0.0.1:9",
                "--headless",
                "--backend-mode",
                "proxy",
                "--force-local"
            ]
        );
    }

    #[test]
    fn parse_ngrok_url_cases() {
        let json = r#"{"lvl":"info","msg":"started tunnel","obj":"tunnels","name":"command_line","addr":"http://127.0.0.1:1","url":"https://Abc-1.ngrok-free.app"}"#;
        let cases: [(&str, Option<&str>, Option<&str>); 12] = [
            (json, None, Some("https://abc-1.ngrok-free.app")),
            (
                "t=2026-01-01 lvl=info msg=\"started tunnel\" addr=http://127.0.0.1:1 url=https://abc-1.ngrok-free.app",
                None,
                Some("https://abc-1.ngrok-free.app"),
            ),
            (
                "lvl=info url=\"https://x.ngrok.app\"",
                None,
                Some("https://x.ngrok.app"),
            ),
            (
                "url=https://demo.example.com",
                Some("demo.example.com"),
                Some("https://demo.example.com"),
            ),
            ("url=https://demo.example.com", None, None),
            (
                r#"{"url":"https://x.ngrok-free.dev:443/path"}"#,
                None,
                Some("https://x.ngrok-free.dev"),
            ),
            ("url=https://x.ngrok.io", None, Some("https://x.ngrok.io")),
            ("url=https://evil.example.com", None, None),
            ("url=http://x.ngrok-free.app", None, None),
            ("lvl=info addr=http://127.0.0.1:1", None, None),
            ("garbage {not json\n\n", None, None),
            ("public_url=https://x.ngrok.app", None, None),
        ];
        for (log, domain, expected) in cases {
            assert_eq!(
                parse_ngrok_url(log, domain).as_deref(),
                expected,
                "{log:?} {domain:?}"
            );
        }
    }

    #[test]
    fn parse_zrok_url_cases() {
        let banner = "[   0.512]    INFO main: access your zrok share at the following endpoints:";
        for (log, expected) in [
            (
                format!("{banner}\n https://stubzrok.share.zrok.io\n"),
                Some("https://stubzrok.share.zrok.io"),
            ),
            (
                format!("{banner}\n https://one.share.zrok.io\nhttps://two.share.zrok.io\n"),
                Some("https://one.share.zrok.io"),
            ),
            (format!("{banner}\n"), None),
            (
                "https://early.share.zrok.io before the banner".to_owned(),
                None,
            ),
        ] {
            assert_eq!(parse_zrok_url(&log).as_deref(), expected, "{log:?}");
        }
    }

    #[test]
    fn last_ngrok_error_cases() {
        let cases: [(&str, Option<&str>); 6] = [
            (
                "{\"lvl\":\"info\",\"msg\":\"ok\"}\n{\"lvl\":\"crit\",\"msg\":\"command failed\",\"err\":\"session closed\"}",
                Some("session closed"),
            ),
            (
                "{\"lvl\":\"eror\",\"msg\":\"failed to reconnect\"}",
                Some("failed to reconnect"),
            ),
            (
                "ERROR:  authentication failed: Your account could not be authenticated ERR_NGROK_105\n",
                Some("authentication failed: Your account could not be authenticated ERR_NGROK_105"),
            ),
            (
                "ERROR:\nERROR:\n\n  endpoint is already online\nERROR:  ERR_NGROK_334\n",
                Some("ERR_NGROK_334"),
            ),
            (
                "ERROR: plain text\n{\"lvl\":\"crit\",\"err\":\"json wins\"}\n",
                Some("json wins"),
            ),
            ("{\"lvl\":\"info\",\"msg\":\"started tunnel\"}\nnoise", None),
        ];
        for (log, expected) in cases {
            assert_eq!(last_ngrok_error(log).as_deref(), expected, "{log:?}");
        }
        let bare = "ERROR:\n\n  the endpoint is already online\n\nERR_NGROK_334\n";
        assert_eq!(
            last_ngrok_error(bare).as_deref(),
            Some("the endpoint is already online (ERR_NGROK_334)")
        );
        let once =
            last_ngrok_error("ERROR: bad token ERR_NGROK_105\nERR_NGROK_105").unwrap_or_default();
        assert_eq!(once.matches("ERR_NGROK_105").count(), 1);
    }

    #[test]
    fn last_zrok_error_cases() {
        let cases: [(&str, Option<&str>); 5] = [
            (
                "[ERROR]: unable to create share (no environment)",
                Some("unable to create share (no environment)"),
            ),
            (
                "[   0.100]   ERROR main.run: unable to load environment\nINFO later",
                Some("main.run: unable to load environment"),
            ),
            ("INFO all good\nERRORS are not a token", None),
            ("[ERROR]:   ", None),
            ("", None),
        ];
        for (log, expected) in cases {
            assert_eq!(last_zrok_error(log).as_deref(), expected, "{log:?}");
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
