//! Tool update polling and the daemon's tool update cache.
//!
//! The cache (`DaemonState::tool_updates`) maps a tool id to the newest version
//! published at the last poll. [`poll_and_refresh`] fills it on the same
//! 12-hour tick as the PHP and Node checks, and `ListTools` / `TunnelStatus`
//! compare it to what is installed without touching the network. Node is not
//! cached here: its updates come from `node_updates`. `cloudflared` is a tool
//! like any other, so `TunnelStatus` reads its entry under its tool id.
//!
//! Notify-only: nothing is installed or updated by the poll.

use yerd_php::Downloader;

use crate::state::DaemonState;
use crate::tools::Tool;

/// Fetch the latest version of every installed managed tool and refresh the
/// cache. Uninstalled tools (including a `PATH`-only `cloudflared`) are never
/// fetched. A failed lookup keeps that tool's previous entry.
pub async fn poll_and_refresh(state: &DaemonState, dl: &dyn Downloader) {
    for tool in Tool::ALL {
        if tool == Tool::Node || crate::tools::installed_version(&state.dirs, tool, None).is_none()
        {
            continue;
        }
        match crate::tools::latest(tool, &state.dirs, dl).await {
            Ok(Some(latest)) => store(state, tool.id(), latest).await,
            Ok(None) => {}
            Err(e) => {
                tracing::debug!(tool = tool.id(), error = %e, "tool update poll failed, keeping cached entry");
            }
        }
    }
}

async fn store(state: &DaemonState, key: &str, latest: String) {
    state
        .tool_updates
        .write()
        .await
        .insert(key.to_owned(), latest);
}

/// The cached latest version for `key`, if any (no network).
pub async fn cached(state: &DaemonState, key: &str) -> Option<String> {
    state.tool_updates.read().await.get(key).cloned()
}

/// Fill `update_available` on each managed, installed tool from the cache (no
/// network). Node is compared against the `node_updates` cache using the
/// default release's version.
pub async fn fill_update_available(state: &DaemonState, tools: &mut [yerd_ipc::ToolStatus]) {
    let cache = state.tool_updates.read().await.clone();
    for status in tools.iter_mut() {
        let Some(tool) = Tool::parse(&status.id) else {
            continue;
        };
        if !status.installed {
            continue;
        }
        status.update_available = if tool == Tool::Node {
            node_update(state, status.version.as_deref()).await
        } else {
            let installed =
                crate::tools::comparable_version(&state.dirs, tool, status.version.as_deref());
            installed
                .zip(cache.get(tool.id()))
                .and_then(|(installed, latest)| crate::tools::newer_version(&installed, latest))
        };
    }
}

async fn node_update(state: &DaemonState, version: Option<&str>) -> Option<String> {
    let bare = version?.trim_start_matches('v');
    let updates = crate::node_updates::cached_updates(state, &[bare]).await;
    updates
        .get(bare)
        .map(|latest| format!("v{}", latest.trim_start_matches('v')))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::test_support::{state_in, UrlDl};

    const COMPOSER_VERSIONS: &str = r#"{"stable":[{"path":"/download/2.8.12/composer.phar","version":"2.8.12","min-php":70205}]}"#;
    const CLOUDFLARED_RELEASE: &str = r#"{"tag_name":"2025.11.1","assets":[]}"#;

    fn mark_installed(state: &DaemonState, tool: Tool, version: &str) {
        crate::test_support::fake_php(&state.dirs, "8.4");
        let dir = state.dirs.data.join("tools").join(tool.id());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".version"), version).unwrap();
    }

    fn online() -> UrlDl {
        UrlDl::default()
            .with("https://getcomposer.org/versions", COMPOSER_VERSIONS)
            .with(
                "https://repo.packagist.org/p2/laravel/installer.json",
                r#"{"packages":{"laravel/installer":[{"version":"v5.33.0"}]}}"#,
            )
            .with(
                "https://api.github.com/repos/cloudflare/cloudflared/releases/latest",
                CLOUDFLARED_RELEASE,
            )
    }

    async fn listed(state: &DaemonState) -> Vec<yerd_ipc::ToolStatus> {
        let mut tools = crate::tools::list_status(&state.dirs, None);
        fill_update_available(state, &mut tools).await;
        tools
    }

    fn by_id<'a>(tools: &'a [yerd_ipc::ToolStatus], id: &str) -> &'a yerd_ipc::ToolStatus {
        tools.iter().find(|t| t.id == id).unwrap()
    }

    #[tokio::test]
    async fn poll_reports_older_managed_installs_only() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        mark_installed(&state, Tool::Composer, "2.8.1");
        poll_and_refresh(&state, &online()).await;

        assert_eq!(cached(&state, "composer").await.as_deref(), Some("2.8.12"));
        assert_eq!(cached(&state, "laravel").await, None);
        let tools = listed(&state).await;
        assert_eq!(
            by_id(&tools, "composer").update_available.as_deref(),
            Some("2.8.12")
        );
        assert_eq!(by_id(&tools, "laravel").update_available, None);
        assert_eq!(by_id(&tools, "bun").update_available, None);

        mark_installed(&state, Tool::Composer, "2.8.12");
        let tools = listed(&state).await;
        assert_eq!(by_id(&tools, "composer").update_available, None);
    }

    #[tokio::test]
    async fn laravel_compares_installed_php_not_the_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        mark_installed(&state, Tool::Laravel, "installed");
        poll_and_refresh(&state, &online()).await;
        assert_eq!(
            by_id(&listed(&state).await, "laravel").update_available,
            None
        );

        let composer = state
            .dirs
            .data
            .join("tools")
            .join("laravel")
            .join("vendor")
            .join("composer");
        std::fs::create_dir_all(&composer).unwrap();
        std::fs::write(
            composer.join("installed.php"),
            "<?php return array(\n    'root' => array(\n        'pretty_version' => 'v5.32.0',\n    ),\n    'versions' => array(),\n);\n",
        )
        .unwrap();
        assert_eq!(
            by_id(&listed(&state).await, "laravel")
                .update_available
                .as_deref(),
            Some("v5.33.0")
        );
    }

    #[tokio::test]
    async fn failed_poll_keeps_the_previous_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        mark_installed(&state, Tool::Composer, "2.8.1");
        poll_and_refresh(&state, &online()).await;
        poll_and_refresh(&state, &UrlDl::default()).await;
        assert_eq!(cached(&state, "composer").await.as_deref(), Some("2.8.12"));
        assert_eq!(
            by_id(&listed(&state).await, "composer")
                .update_available
                .as_deref(),
            Some("2.8.12")
        );
    }

    #[tokio::test]
    async fn managed_cloudflared_reports_a_newer_release() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        crate::tools::cloudflared::install_binary_for_test(&state.dirs, "2024.6.1", b"bin");
        poll_and_refresh(&state, &online()).await;
        assert_eq!(
            cached(&state, "cloudflared").await.as_deref(),
            Some("2025.11.1")
        );
        assert_eq!(
            by_id(&listed(&state).await, "cloudflared")
                .update_available
                .as_deref(),
            Some("2025.11.1")
        );
        let status = crate::tunnel::cloudflared_status(&state).await;
        assert_eq!(status.update_available.as_deref(), Some("2025.11.1"));
    }

    #[tokio::test]
    async fn no_managed_cloudflared_is_never_fetched() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        poll_and_refresh(&state, &online()).await;
        assert_eq!(cached(&state, "cloudflared").await, None);
        let status = crate::tunnel::cloudflared_status(&state).await;
        assert_eq!(status.update_available, None);
    }
}
