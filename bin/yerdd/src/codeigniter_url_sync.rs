//! Keeps a `CodeIgniter` 4 site's `app.baseURL` (in its `.env`) in step with
//! the scheme and domain yerd serves it on - the counterpart of
//! [`crate::wordpress_url_sync`], run from the same post-mutation hook. The
//! create wizard writes `app.baseURL` once; without this, toggling HTTPS or
//! changing the primary domain afterwards left every URL the app generates
//! pointing at the old address.
//!
//! Only an active `app.baseURL` line is rewritten: a commented-out or missing
//! setting is the user's choice and left alone. Best-effort: a failed write only
//! logs a warning and never fails the mutation it's attached to.

use std::path::Path;

use yerd_core::{Site, SiteRouter};

use crate::public_url::UrlSyncOutcome;
use crate::state::DaemonState;

/// Post-mutation hook (also run once for a site renamed on upgrade): if `site`
/// is a `CodeIgniter` 4 project with an active `app.baseURL` in its `.env`,
/// point it at the site's current URL. `Failed` when the site has no concrete
/// host right now or the write failed; `NotApplicable` when it is not such a
/// project or the line already matches.
pub async fn sync_base_url(site: &Site, state: &DaemonState) -> UrlSyncOutcome {
    let root = site.document_root();
    if !root.join("spark").is_file() {
        return UrlSyncOutcome::NotApplicable;
    }
    let dotenv = root.join(".env");
    let _serial = state.url_sync.lock().await;
    let current = match tokio::fs::read_to_string(&dotenv).await {
        Ok(current) => current,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return UrlSyncOutcome::NotApplicable;
        }
        Err(e) => {
            tracing::warn!(
                site = %site.name(),
                error = %e,
                "couldn't read CodeIgniter .env to sync app.baseURL"
            );
            return UrlSyncOutcome::Failed;
        }
    };

    let Some((host, secure)) = advertised_host(&*state.router.read().await, site.name()) else {
        return UrlSyncOutcome::Failed;
    };
    let url = crate::public_url::site_url(state, &host, secure).await;
    let Some(updated) = rewrite_base_url(&current, &format!("{url}/")) else {
        return UrlSyncOutcome::NotApplicable;
    };
    if let Err(e) = replace_file(&dotenv, &updated).await {
        tracing::warn!(
            site = %site.name(),
            error = %e,
            "couldn't sync CodeIgniter app.baseURL after a site change"
        );
        return UrlSyncOutcome::Failed;
    }
    UrlSyncOutcome::Done
}

/// Replace `path`'s contents atomically: write a sibling temporary file with
/// the original's permissions, then rename it over the target (resolving a
/// symlinked `.env` first so the link survives). On failure the original is
/// untouched and the temporary file is removed.
async fn replace_file(path: &Path, contents: &str) -> std::io::Result<()> {
    let target = tokio::fs::canonicalize(path).await?;
    let permissions = tokio::fs::metadata(&target).await?.permissions();
    let mut tmp_name = target.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".yerd-tmp");
    let tmp = target.with_file_name(tmp_name);
    let written = async {
        tokio::fs::write(&tmp, contents).await?;
        tokio::fs::set_permissions(&tmp, permissions).await?;
        tokio::fs::rename(&tmp, &target).await
    }
    .await;
    if written.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    written
}

/// The host and secure flag to advertise for `name`, read from the live router
/// rather than the hook's snapshot so a hook that finishes after a later
/// mutation cannot write back a stale scheme. `None` when the site is gone or
/// its primary FQDN routes to a different site (a shadowed apex leaves it
/// wildcard-only): no concrete host reaches it, so the `.env` is left alone.
/// Same guard as `tunnel::resolve_site`.
fn advertised_host(router: &SiteRouter, name: &str) -> Option<(String, bool)> {
    let secure = router.get(name)?.secure();
    let host = router.primary_fqdn(name);
    (router.resolve(&host).map(Site::name) == Some(name)).then_some((host, secure))
}

/// Replace the first active `app.baseURL` line with `base_url`. Pure - `None`
/// when there is no active line or it already matches.
fn rewrite_base_url(env: &str, base_url: &str) -> Option<String> {
    let wanted = format!("app.baseURL = '{base_url}'");
    let mut replaced = false;
    let mut changed = false;
    let mut lines: Vec<&str> = Vec::new();
    for line in env.lines() {
        if !replaced && is_active_base_url(line) {
            replaced = true;
            changed = line.trim() != wanted;
            lines.push(&wanted);
        } else {
            lines.push(line);
        }
    }
    if !changed {
        return None;
    }
    let mut out = lines.join("\n");
    if env.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

fn is_active_base_url(line: &str) -> bool {
    let line = line.trim_start();
    !line.starts_with('#')
        && line
            .split_once('=')
            .is_some_and(|(key, _)| key.trim() == "app.baseURL")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use yerd_config::{Config, DomainDelta};
    use yerd_core::{Domain, PhpVersion};

    fn router(sites: &[(&str, bool)], deltas: &[(&str, &str)]) -> SiteRouter {
        let mut cfg = Config::default();
        cfg.tld = yerd_core::Tld::new("test").unwrap();
        for (site, domain) in deltas {
            cfg.domains.linked.insert(
                (*site).into(),
                DomainDelta {
                    added: vec![Domain::parse_subpart(domain).unwrap()],
                    suppressed: vec![],
                    primary: None,
                },
            );
        }
        let sites = sites
            .iter()
            .map(|(name, secure)| {
                let mut s =
                    Site::linked(name, format!("/srv/{name}"), PhpVersion::new(8, 3)).unwrap();
                s.set_secure(*secure);
                s
            })
            .collect();
        crate::site_domains::build(&cfg, sites)
    }

    #[test]
    fn advertised_host_is_the_primary_fqdn_with_the_live_secure_flag() {
        let r = router(&[("blog", true), ("shop", false)], &[]);
        assert_eq!(
            advertised_host(&r, "blog"),
            Some(("blog.test".to_owned(), true))
        );
        assert_eq!(
            advertised_host(&r, "shop"),
            Some(("shop.test".to_owned(), false))
        );
    }

    #[test]
    fn advertised_host_is_none_when_the_primary_routes_to_another_site() {
        let r = router(&[("a", false), ("b", false)], &[("a", "*.a"), ("b", "a")]);
        assert_eq!(r.resolve("a.test").map(Site::name), Some("b"));
        assert_eq!(advertised_host(&r, "a"), None);
    }

    #[test]
    fn advertised_host_is_none_for_an_unknown_site() {
        let r = router(&[("blog", false)], &[]);
        assert_eq!(advertised_host(&r, "gone"), None);
    }

    #[tokio::test]
    async fn replace_file_swaps_contents_and_leaves_no_temporary_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dotenv = tmp.path().join(".env");
        std::fs::write(&dotenv, "old\n").unwrap();
        replace_file(&dotenv, "new\n").await.unwrap();
        assert_eq!(std::fs::read_to_string(&dotenv).unwrap(), "new\n");
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn replace_file_fails_without_creating_a_missing_target() {
        let tmp = tempfile::tempdir().unwrap();
        let dotenv = tmp.path().join(".env");
        assert!(replace_file(&dotenv, "new\n").await.is_err());
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn rewrites_the_active_base_url_line() {
        let env = "CI_ENVIRONMENT = development\napp.baseURL = 'http://blog.test/'\n";
        assert_eq!(
            rewrite_base_url(env, "https://blog.test/").unwrap(),
            "CI_ENVIRONMENT = development\napp.baseURL = 'https://blog.test/'\n"
        );
    }

    #[test]
    fn keeps_a_missing_trailing_newline_missing() {
        assert_eq!(
            rewrite_base_url("app.baseURL = 'http://a.test/'", "https://a.test/").unwrap(),
            "app.baseURL = 'https://a.test/'"
        );
    }

    #[test]
    fn leaves_commented_missing_and_matching_settings_alone() {
        for env in [
            "# app.baseURL = 'http://blog.test/'\n",
            "CI_ENVIRONMENT = development\n",
            "app.baseURL = 'https://blog.test/'\n",
            "",
        ] {
            assert_eq!(rewrite_base_url(env, "https://blog.test/"), None, "{env:?}");
        }
    }

    #[test]
    fn only_the_first_active_line_is_rewritten() {
        let env = "app.baseURL = 'http://a.test/'\napp.baseURL = 'http://b.test/'\n";
        assert_eq!(
            rewrite_base_url(env, "https://a.test/").unwrap(),
            "app.baseURL = 'https://a.test/'\napp.baseURL = 'http://b.test/'\n"
        );
    }

    #[test]
    fn is_active_base_url_ignores_similar_keys() {
        for (line, expected) in [
            ("app.baseURL = ''", true),
            ("  app.baseURL=''", true),
            ("# app.baseURL = ''", false),
            ("app.baseURLs = ''", false),
            ("app.allowedHostnames = []", false),
        ] {
            assert_eq!(is_active_base_url(line), expected, "{line:?}");
        }
    }
}
