//! Pure share preflight checks.
//!
//! Spot site setups that break for public visitors before a share goes live: a
//! running Vite dev server (assets would load from the sharer's machine) and a
//! `SESSION_DOMAIN` pinned to the local TLD (session cookies would be rejected on
//! the public hostname). The daemon gathers the facts (a file check and the
//! `.env` text) and this module turns them into human-readable warnings. Warnings
//! never block a share.

/// Warning when the site's web root holds Vite's `hot` file.
pub const VITE_HOT_WARNING: &str =
    "Vite dev server is running (the web root has a `hot` file), so \
     visitors' browsers will try to load assets from your machine; stop it or run a production \
     build before sharing";

/// The facts a preflight check needs about one site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareFacts<'a> {
    /// Whether `<served root>/hot` exists (Vite's dev-server marker).
    pub vite_hot: bool,
    /// The site's `.env` contents, when it has one.
    pub dotenv: Option<&'a str>,
    /// The local TLD without a leading dot, e.g. `test`.
    pub tld: &'a str,
}

/// The preflight warnings for a share, in a stable order. Empty when nothing
/// looks wrong.
#[must_use]
pub fn preflight(facts: &ShareFacts<'_>) -> Vec<String> {
    let mut warnings = Vec::new();
    if facts.vite_hot {
        warnings.push(VITE_HOT_WARNING.to_owned());
    }
    if let Some(domain) = facts
        .dotenv
        .and_then(|env| dotenv_value(env, "SESSION_DOMAIN"))
        .filter(|domain| is_local_domain(domain, facts.tld))
    {
        warnings.push(format!(
            "SESSION_DOMAIN in .env is {domain}, so session cookies (and logins) won't work on \
             the public hostname; unset it or set it to null while sharing"
        ));
    }
    warnings
}

/// The effective value of `key` in `.env` text: the last assignment wins, an
/// optional `export ` prefix is accepted, surrounding quotes are removed, and an
/// unquoted ` #` starts a comment. A blank or `null` value counts as unset.
fn dotenv_value(env: &str, key: &str) -> Option<String> {
    env.lines()
        .rev()
        .find_map(|line| assignment(line, key))
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("null"))
}

/// The value assigned to `key` on `line`, or `None` when the line is blank, a
/// comment, or assigns a different key.
fn assignment(line: &str, key: &str) -> Option<String> {
    let line = line.trim();
    if line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").map_or(line, str::trim_start);
    let (name, raw) = line.split_once('=')?;
    if name.trim() != key {
        return None;
    }
    Some(unquote(raw.trim()))
}

/// Strip matching quotes (up to the closing quote) or an unquoted trailing
/// comment from a raw `.env` value.
fn unquote(raw: &str) -> String {
    for quote in ['"', '\''] {
        if let Some(inner) = raw.strip_prefix(quote) {
            return inner
                .split_once(quote)
                .map_or(inner, |(value, _)| value)
                .to_owned();
        }
    }
    let value = raw
        .find(" #")
        .or_else(|| raw.find("\t#"))
        .and_then(|at| raw.get(..at))
        .unwrap_or(raw);
    value.trim().to_owned()
}

/// Whether `domain` (an optional leading dot) is the local TLD itself or a name
/// under it, compared case-insensitively.
fn is_local_domain(domain: &str, tld: &str) -> bool {
    let domain = domain
        .strip_prefix('.')
        .unwrap_or(domain)
        .to_ascii_lowercase();
    let tld = tld.strip_prefix('.').unwrap_or(tld).to_ascii_lowercase();
    !tld.is_empty() && (domain == tld || domain.ends_with(&format!(".{tld}")))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn warnings(vite_hot: bool, dotenv: Option<&str>) -> Vec<String> {
        preflight(&ShareFacts {
            vite_hot,
            dotenv,
            tld: "test",
        })
    }

    #[test]
    fn vite_hot_file_table() {
        assert_eq!(warnings(true, None), vec![VITE_HOT_WARNING.to_owned()]);
        assert!(warnings(false, None).is_empty());
    }

    #[test]
    fn session_domain_table() {
        let cases: &[(&str, bool)] = &[
            ("SESSION_DOMAIN=.app.test", true),
            ("SESSION_DOMAIN=\"app.test\"", true),
            ("SESSION_DOMAIN='APP.TEST'", true),
            ("SESSION_DOMAIN=test", true),
            ("export SESSION_DOMAIN=app.test", true),
            ("SESSION_DOMAIN=app.test # local only", true),
            ("export SESSION_DOMAIN=null", false),
            ("SESSION_DOMAIN=", false),
            ("SESSION_DOMAIN=\"\"", false),
            ("#SESSION_DOMAIN=app.test", false),
            ("SESSION_DOMAIN=example.com", false),
            ("SESSION_DOMAIN=app.local", false),
            ("SESSION_DOMAIN=apptest", false),
            ("APP_URL=https://app.test", false),
            ("SESSION_DOMAIN=app.test\nSESSION_DOMAIN=null", false),
            ("SESSION_DOMAIN=null\nSESSION_DOMAIN=app.test", true),
        ];
        for (env, expected) in cases {
            let found = warnings(false, Some(env));
            assert_eq!(found.len(), usize::from(*expected), "{env:?}: {found:?}");
            if *expected {
                assert!(
                    found[0].starts_with("SESSION_DOMAIN in .env is "),
                    "{env:?}"
                );
            }
        }
    }

    #[test]
    fn session_domain_warning_names_the_value() {
        let found = warnings(false, Some("SESSION_DOMAIN=.app.test"));
        assert!(found[0].contains("is .app.test,"), "{found:?}");
    }

    #[test]
    fn both_warnings_keep_a_stable_order() {
        let found = warnings(true, Some("SESSION_DOMAIN=app.test"));
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], VITE_HOT_WARNING);
        assert!(found[1].starts_with("SESSION_DOMAIN"));
    }

    #[test]
    fn is_local_domain_handles_a_dotted_tld() {
        assert!(is_local_domain("app.test", ".test"));
        assert!(!is_local_domain("app.test", ""));
    }
}
