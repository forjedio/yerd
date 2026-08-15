//! Pure matching of per-site routing rules against a request path.
//!
//! The mirror of `yerd_core::match_rule` for [`RouteRule`], which resolves to a
//! local file rather than an HTTP upstream. Matching is the whole of the pure
//! half: whether the matched target is a PHP script or a static document is
//! derived at the dispatch site from `try_files::is_php_source`, so nothing
//! here needs to know.

use yerd_core::RouteRule;

/// Longest-prefix match of `path` against `rules`, or `None` when none match.
///
/// Callers pass the raw, case-sensitive, percent-encoded `uri.path()` with no
/// normalization, exactly as `yerd_core::match_rule` does: an under-match (an
/// encoded path failing to match) is acceptable for a dev tool, and the matcher
/// never over-matches. Ties cannot occur because duplicate prefixes per site are
/// rejected when the rule is added.
#[must_use]
pub fn match_route<'a>(rules: &'a [RouteRule], path: &str) -> Option<&'a RouteRule> {
    rules
        .iter()
        .filter(|r| r.matches_path(path))
        .max_by_key(|r| r.prefix().len())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn rule(prefix: &str, target: &str) -> RouteRule {
        RouteRule::new(prefix, target).unwrap()
    }

    #[test]
    fn matches_are_boundary_correct() {
        let rules = vec![rule("/api", "api/index.php")];
        let cases: &[(&str, bool)] = &[
            ("/api", true),
            ("/api/", true),
            ("/api/user/login", true),
            ("/apix", false),
            ("/ap", false),
            ("/", false),
        ];
        for (path, want) in cases {
            assert_eq!(match_route(&rules, path).is_some(), *want, "path {path:?}");
        }
    }

    #[test]
    fn longest_prefix_wins() {
        let rules = vec![
            rule("/api", "api/index.php"),
            rule("/api/admin", "api/admin/index.php"),
        ];
        assert_eq!(
            match_route(&rules, "/api/admin/x").unwrap().target(),
            "api/admin/index.php"
        );
        assert_eq!(
            match_route(&rules, "/api/other").unwrap().target(),
            "api/index.php"
        );
        assert!(match_route(&rules, "/nope").is_none());
    }

    #[test]
    fn root_rule_is_catch_all_and_loses_to_a_longer_prefix() {
        let rules = vec![rule("/", "index.html"), rule("/api", "api/index.php")];
        assert_eq!(
            match_route(&rules, "/dashboard/settings").unwrap().target(),
            "index.html"
        );
        assert_eq!(
            match_route(&rules, "/api/user").unwrap().target(),
            "api/index.php"
        );
    }

    #[test]
    fn empty_rule_set_never_matches() {
        assert!(match_route(&[], "/anything").is_none());
    }
}
