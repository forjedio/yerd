//! Pure per-site static routing rules.
//!
//! A [`RouteRule`] maps a URI path prefix to a **local** target under the site's
//! served root: a PHP script (`api/index.php`) for a nested front controller, or
//! a static document (`index.html`) for SPA history-API routing. The proxy
//! applies a rule only when the request matched no real file, so the semantics
//! are nginx's `try_files $uri $uri/ <target>`.
//!
//! This is the local-file sibling of [`ProxyRule`](crate::ProxyRule), which
//! forwards to an HTTP upstream instead. Both are pure data with no serde impls:
//! `yerd-config` and `yerd-ipc` own their own wire structs.
//!
//! Validation deliberately duplicates [`ProxyRule::new`](crate::ProxyRule::new)'s
//! prefix checks rather than sharing a helper, so neither shipped type has to
//! change; each keeps its own table tests to catch drift.

use std::path::Path;

use crate::error::{CoreError, RouteRuleErrorReason};
use crate::site::is_safe_relative;

/// A path-prefix routing rule attached to a PHP site: requests under
/// [`Self::prefix`] that match no real file are handled by [`Self::target`], a
/// path relative to the site's served root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRule {
    prefix: String,
    target: String,
}

impl RouteRule {
    /// Validates `prefix` (absolute; no `..` component; no control chars, space,
    /// `?`, or `#`, none of which can appear in `uri.path()`) and normalizes a
    /// trailing slash away (`/api/` → `/api`; root stays `/`).
    ///
    /// Validates `target` as a safe relative path: non-empty, no control
    /// characters, and no root, drive prefix, or `..` component, so it can only
    /// ever resolve to a descendant of the served root. There is no
    /// trailing-slash rule for the target because [`Path`] normalizes one away.
    /// Containment is still re-checked against the real filesystem at request
    /// time; this is the first line of defence, not the only one.
    pub fn new(prefix: &str, target: &str) -> Result<Self, CoreError> {
        let prefix_err = |r: RouteRuleErrorReason| CoreError::InvalidRouteRule {
            input: prefix.to_owned(),
            reason: r,
        };
        if prefix.is_empty() {
            return Err(prefix_err(RouteRuleErrorReason::EmptyPrefix));
        }
        if !prefix.starts_with('/') {
            return Err(prefix_err(RouteRuleErrorReason::PrefixNotAbsolute));
        }
        if prefix
            .chars()
            .any(|c| c.is_control() || c == ' ' || c == '?' || c == '#')
        {
            return Err(prefix_err(RouteRuleErrorReason::PrefixContainsControl));
        }
        if prefix.split('/').any(|seg| seg == "..") {
            return Err(prefix_err(RouteRuleErrorReason::PrefixContainsDotDot));
        }

        let target_err = |r: RouteRuleErrorReason| CoreError::InvalidRouteRule {
            input: target.to_owned(),
            reason: r,
        };
        if target.is_empty() {
            return Err(target_err(RouteRuleErrorReason::EmptyTarget));
        }
        if target.chars().any(char::is_control) || !is_safe_relative(Path::new(target)) {
            return Err(target_err(RouteRuleErrorReason::InvalidTarget));
        }

        let mut normalized = prefix.to_owned();
        while normalized.len() > 1 && normalized.ends_with('/') {
            normalized.pop();
        }
        Ok(Self {
            prefix: normalized,
            target: target.to_owned(),
        })
    }

    /// The normalized path prefix (e.g. `/api`; root is `/`).
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The target path, relative to the site's served root.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Whether this rule matches `path`. Boundary-correct: `/api` matches `/api`
    /// and `/api/x` but not `/apix`. Root (`/`) is a catch-all.
    #[must_use]
    pub fn matches_path(&self, path: &str) -> bool {
        if self.prefix == "/" {
            return path.starts_with('/');
        }
        path == self.prefix
            || path
                .strip_prefix(self.prefix.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    }
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
    fn normalizes_trailing_slash_and_keeps_root() {
        let cases: &[(&str, &str)] = &[
            ("/api/", "/api"),
            ("/api", "/api"),
            ("/api///", "/api"),
            ("/", "/"),
            ("/a/b/", "/a/b"),
        ];
        for (input, want) in cases {
            let r = RouteRule::new(input, "api/index.php").unwrap();
            assert_eq!(r.prefix(), *want, "prefix {input:?}");
        }
    }

    #[test]
    fn rejects_bad_prefixes_with_reasons() {
        let cases: &[(&str, RouteRuleErrorReason)] = &[
            ("", RouteRuleErrorReason::EmptyPrefix),
            ("api", RouteRuleErrorReason::PrefixNotAbsolute),
            ("/x\ty", RouteRuleErrorReason::PrefixContainsControl),
            ("/x y", RouteRuleErrorReason::PrefixContainsControl),
            ("/x?y", RouteRuleErrorReason::PrefixContainsControl),
            ("/x#y", RouteRuleErrorReason::PrefixContainsControl),
            ("/a/../b", RouteRuleErrorReason::PrefixContainsDotDot),
        ];
        for (input, want) in cases {
            let err = RouteRule::new(input, "index.html").unwrap_err();
            match err {
                CoreError::InvalidRouteRule { reason, .. } => {
                    assert_eq!(reason, *want, "prefix {input:?}");
                }
                other => panic!("expected InvalidRouteRule for {input:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_bad_targets_with_reasons() {
        let cases: &[(&str, RouteRuleErrorReason)] = &[
            ("", RouteRuleErrorReason::EmptyTarget),
            ("/etc/passwd", RouteRuleErrorReason::InvalidTarget),
            ("../secret.php", RouteRuleErrorReason::InvalidTarget),
            ("api/../../x", RouteRuleErrorReason::InvalidTarget),
            ("api/\tindex.php", RouteRuleErrorReason::InvalidTarget),
            ("api/\0index.php", RouteRuleErrorReason::InvalidTarget),
        ];
        for (input, want) in cases {
            let err = RouteRule::new("/api", input).unwrap_err();
            match err {
                CoreError::InvalidRouteRule { reason, .. } => {
                    assert_eq!(reason, *want, "target {input:?}");
                }
                other => panic!("expected InvalidRouteRule for {input:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn accepts_plain_relative_targets() {
        for target in [
            "index.html",
            "api/index.php",
            "a/b/c/index.php",
            "./index.html",
        ] {
            let r = RouteRule::new("/api", target).unwrap();
            assert_eq!(r.target(), target);
        }
    }

    #[test]
    fn matches_path_is_boundary_correct() {
        let r = RouteRule::new("/api", "api/index.php").unwrap();
        let cases: &[(&str, bool)] = &[
            ("/api", true),
            ("/api/", true),
            ("/api/user/login", true),
            ("/apix", false),
            ("/ap", false),
            ("/", false),
            ("/other/api", false),
        ];
        for (path, want) in cases {
            assert_eq!(r.matches_path(path), *want, "path {path:?}");
        }
    }

    #[test]
    fn root_rule_is_catch_all() {
        let r = RouteRule::new("/", "index.html").unwrap();
        assert!(r.matches_path("/"));
        assert!(r.matches_path("/dashboard/settings"));
        assert!(r.matches_path("/assets/app.js"));
    }
}
