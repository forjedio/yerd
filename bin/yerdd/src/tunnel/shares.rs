//! Per-site shares through any tunnel provider, and the manager key scheme
//! behind them.
//!
//! A site holds at most one share per provider, each its own supervised
//! instance: the Cloudflare quick link is keyed by the site name (unchanged),
//! ngrok by `<site>@ngrok` and zrok by `<site>@zrok`. Site names are
//! `[a-z0-9-]`, so a suffixed key never collides with a site or with the named
//! tunnel's `@named`. Every share points at the same loopback origin listener,
//! and both ngrok and zrok forward the public `Host`, so their captured URLs
//! route (and are password-gated) exactly like a quick link's.
//!
//! The ngrok authtoken reaches the agent only through `NGROK_AUTHTOKEN` in its
//! cleared environment, never its argv, and is never logged. ngrok runs with
//! Yerd's own `{data}/tunnel/ngrok.yml`, so the user's ngrok config is never
//! read. zrok runs in Yerd's environment (`HOME={data}/tunnel/zrok`). Both get
//! the outbound proxy env.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use yerd_core::tunnel_provider::TunnelProvider;
use yerd_ipc::{ErrorCode, Response};
use yerd_platform::PlatformDirs;
use yerd_tunnel::manager::TunnelSnapshot;
use yerd_tunnel::providers::{
    ngrok_share_args, ngrok_version_supported, render_ngrok_config, zrok_share_args,
    NGROK_AUTHTOKEN_ENV, NGROK_MIN_VERSION,
};
use yerd_tunnel::TunnelKind;

use super::install;
use super::providers::{managed_binary, resolve_binary_in, tool_for, zrok_enabled, zrok_home};
use crate::state::DaemonState;
use crate::tools::{self, Tool};

const NGROK_SUFFIX: &str = "@ngrok";
const ZROK_SUFFIX: &str = "@zrok";

/// The manager key of `site`'s share through `provider`.
pub(crate) fn share_key(provider: TunnelProvider, site: &str) -> String {
    match provider {
        TunnelProvider::Cloudflare => site.to_owned(),
        TunnelProvider::Ngrok => format!("{site}{NGROK_SUFFIX}"),
        TunnelProvider::Zrok => format!("{site}{ZROK_SUFFIX}"),
    }
}

/// The site a snapshot shares: its key without the ngrok/zrok suffix, or the
/// key itself for the Cloudflare kinds.
pub(crate) fn site_of(snapshot: &TunnelSnapshot) -> &str {
    let suffix = match snapshot.kind {
        TunnelKind::Ngrok { .. } => NGROK_SUFFIX,
        TunnelKind::Zrok => ZROK_SUFFIX,
        TunnelKind::Quick | TunnelKind::Named => return &snapshot.site,
    };
    snapshot.site.strip_suffix(suffix).unwrap_or(&snapshot.site)
}

/// The provider serving an instance of `kind`.
pub(crate) fn provider_of(kind: TunnelKind) -> TunnelProvider {
    match kind {
        TunnelKind::Quick | TunnelKind::Named => TunnelProvider::Cloudflare,
        TunnelKind::Ngrok { .. } => TunnelProvider::Ngrok,
        TunnelKind::Zrok => TunnelProvider::Zrok,
    }
}

/// Whether `kind` is a per-site share (every kind but the named tunnel).
pub(crate) fn is_per_site(kind: TunnelKind) -> bool {
    !matches!(kind, TunnelKind::Named)
}

fn error(code: ErrorCode, message: impl Into<String>) -> Response {
    Response::Error {
        code,
        message: message.into(),
    }
}

/// The reply when an ngrok or zrok binary is missing.
pub(crate) fn not_installed(provider: TunnelProvider) -> Response {
    let name = provider.as_str();
    error(
        ErrorCode::NotFound,
        format!(
            "{name} is not installed - install it from Tooling or with `yerd install tool {name}`"
        ),
    )
}

/// `StartQuickTunnel`: share `site` through `provider`, or the configured
/// default provider when `None`. A provider that is not ready is an error
/// naming what to set up; there is no fallback to another provider. The
/// user's login-shell `PATH` is resolved only for ngrok or zrok without a
/// managed copy.
pub async fn start_share(
    site: &str,
    provider: Option<TunnelProvider>,
    random_url: bool,
    state: &Arc<DaemonState>,
) -> Response {
    let provider = chosen_provider(provider, state).await;
    let needs_path =
        tool_for(provider).is_some_and(|tool| !managed_binary(&state.dirs, tool).is_file());
    let user_path = if needs_path {
        tools::external::resolve_user_path().await
    } else {
        None
    };
    start_share_in(site, provider, random_url, state, user_path.as_deref()).await
}

/// `provider`, or the configured default provider when `None` (the config
/// lock is released at once).
pub(crate) async fn chosen_provider(
    provider: Option<TunnelProvider>,
    state: &DaemonState,
) -> TunnelProvider {
    match provider {
        Some(provider) => provider,
        None => state.config.lock().await.tunnel.default_provider,
    }
}

/// [`start_share`] for a chosen provider against an explicit user `PATH`.
pub(crate) async fn start_share_in(
    site: &str,
    provider: TunnelProvider,
    random_url: bool,
    state: &Arc<DaemonState>,
    user_path: Option<&[PathBuf]>,
) -> Response {
    tracing::info!(site, provider = %provider.as_str(), "starting share");
    match provider {
        TunnelProvider::Cloudflare => super::start_quick_tunnel(site, state).await,
        TunnelProvider::Ngrok => start_ngrok(site, random_url, state, user_path).await,
        TunnelProvider::Zrok => start_zrok(site, state, user_path).await,
    }
}

/// The canonical site name and the origin URL every share points at.
async fn site_and_origin(state: &DaemonState, site: &str) -> Result<(String, String), Response> {
    let Some((name, _host)) = super::resolve_site(state, site).await else {
        return Err(error(
            ErrorCode::NotFound,
            format!("no site named {site:?}"),
        ));
    };
    let Some(port) = state.tunnel_origin_port else {
        return Err(super::origin_not_running());
    };
    Ok((name, yerd_tunnel::origin_url(port)))
}

/// The outbound proxy env for a share child, read with no config lock held.
async fn proxy_env(state: &DaemonState) -> Vec<(OsString, OsString)> {
    crate::network::plan_env_pairs(&crate::network::child_env_plan(state).await)
}

/// Write `{data}/tunnel/ngrok.yml` (0600, atomically) and return its path.
fn write_ngrok_config(dirs: &PlatformDirs) -> Result<PathBuf, String> {
    let dir = install::tunnel_dir(dirs);
    crate::secure_fs::create_private_dir(&dir)
        .map_err(|e| format!("could not prepare tunnel dir: {e}"))?;
    let path = dir.join("ngrok.yml");
    yerd_php::io::atomic_write::write(&path, render_ngrok_config().as_bytes())
        .map_err(|e| format!("write ngrok config: {e}"))?;
    Ok(path)
}

async fn start_ngrok(
    site: &str,
    random_url: bool,
    state: &Arc<DaemonState>,
    user_path: Option<&[PathBuf]>,
) -> Response {
    let Some(binary) = resolve_binary_in(&state.dirs, Tool::Ngrok, user_path).await else {
        return not_installed(TunnelProvider::Ngrok);
    };
    if let Some(version) = binary
        .version
        .as_deref()
        .filter(|v| !ngrok_version_supported(v))
    {
        let (major, minor, patch) = NGROK_MIN_VERSION;
        return error(
            ErrorCode::InvalidPath,
            format!(
                "ngrok {version} is too old to share through Yerd (needs {major}.{minor}.{patch} or newer) - install Yerd's copy from Tooling"
            ),
        );
    }
    let (token, domain) = {
        let cfg = state.config.lock().await;
        (
            cfg.tunnel.ngrok.authtoken.clone(),
            cfg.tunnel.ngrok.domain.clone(),
        )
    };
    let Some(token) = token.filter(|t| !t.is_empty()) else {
        return error(
            ErrorCode::InvalidPath,
            "ngrok needs an authtoken - set it in Providers or with `yerd tunnel ngrok token`",
        );
    };
    let (name, origin) = match site_and_origin(state, site).await {
        Ok(found) => found,
        Err(resp) => return resp,
    };
    let static_domain = domain.filter(|_| !random_url);
    let key = share_key(TunnelProvider::Ngrok, &name);
    if let Some(domain) = static_domain.as_deref() {
        if let Some(holder) = static_domain_holder(state, &key).await {
            return error(
                ErrorCode::AlreadyExists,
                format!(
                    "the ngrok static domain {domain} is already in use by {holder}'s share - stop it or share without the static domain"
                ),
            );
        }
    }
    let config_path = match write_ngrok_config(&state.dirs) {
        Ok(path) => path,
        Err(message) => return error(ErrorCode::Internal, message),
    };
    let mut env = super::forwarded_env();
    env.push((
        "HOME".into(),
        install::tunnel_dir(&state.dirs).into_os_string(),
    ));
    env.push((NGROK_AUTHTOKEN_ENV.into(), token.into()));
    env.extend(proxy_env(state).await);
    let args = ngrok_share_args(&origin, &config_path, static_domain.as_deref());
    let spec = match super::launch_spec(state, &key, binary.path, args, env) {
        Ok(spec) => spec,
        Err(resp) => return resp,
    };
    super::refresh_share_warnings(state, &name).await;
    let kind = TunnelKind::Ngrok {
        static_domain: static_domain.is_some(),
    };
    super::launch_and_settle(state, &key, &name, spec, kind).await
}

/// The site of another ngrok share (any key but `key`) serving the static
/// domain, in any state. ngrok refuses a second agent on an online endpoint
/// (`ERR_NGROK_334`), so a new static-domain share would only fail.
async fn static_domain_holder(state: &DaemonState, key: &str) -> Option<String> {
    let mut mgr = state.tunnel_manager.lock().await;
    mgr.snapshots()
        .iter()
        .find(|s| {
            s.site != key
                && matches!(
                    s.kind,
                    TunnelKind::Ngrok {
                        static_domain: true
                    }
                )
        })
        .map(|s| site_of(s).to_owned())
}

/// Start a zrok share. The enabled check and the launch happen under
/// `zrok_mutate`, so an enable or disable never interleaves between them; the
/// lock is released before the settle wait so the Providers sheet never
/// blocks on it.
async fn start_zrok(
    site: &str,
    state: &Arc<DaemonState>,
    user_path: Option<&[PathBuf]>,
) -> Response {
    let Some(binary) = resolve_binary_in(&state.dirs, Tool::Zrok, user_path).await else {
        return not_installed(TunnelProvider::Zrok);
    };
    let (name, origin) = match site_and_origin(state, site).await {
        Ok(found) => found,
        Err(resp) => return resp,
    };
    let mut env = super::forwarded_env();
    env.push(("HOME".into(), zrok_home(&state.dirs).into_os_string()));
    env.extend(proxy_env(state).await);
    let key = share_key(TunnelProvider::Zrok, &name);
    let spec = match super::launch_spec(state, &key, binary.path, zrok_share_args(&origin), env) {
        Ok(spec) => spec,
        Err(resp) => return resp,
    };
    super::refresh_share_warnings(state, &name).await;
    let launched = {
        let _guard = state.zrok_mutate.lock().await;
        if !zrok_enabled(&state.dirs) {
            return error(
                ErrorCode::InvalidPath,
                "zrok is not enabled on this machine - enable it in Providers or with `yerd tunnel zrok enable`",
            );
        }
        super::supervise::launch(state, &key, spec, TunnelKind::Zrok).await
    };
    match launched {
        Ok(launched) => super::settle_launched(state, &key, &name, "zrok share", launched).await,
        Err(e) => super::launch_error(TunnelKind::Zrok, &name, &e),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod key_tests {
    use super::*;
    use yerd_tunnel::TunnelState;

    fn snapshot(site: &str, kind: TunnelKind) -> TunnelSnapshot {
        TunnelSnapshot {
            site: site.to_owned(),
            kind,
            state: TunnelState::Running,
            pid: None,
            url: None,
            generation: 1,
            last_error: None,
            failures: 0,
        }
    }

    #[test]
    fn share_keys_round_trip_through_site_of() {
        let ngrok = TunnelKind::Ngrok {
            static_domain: true,
        };
        for (provider, kind, key) in [
            (TunnelProvider::Cloudflare, TunnelKind::Quick, "app"),
            (TunnelProvider::Ngrok, ngrok, "app@ngrok"),
            (TunnelProvider::Zrok, TunnelKind::Zrok, "app@zrok"),
        ] {
            assert_eq!(share_key(provider, "app"), key);
            assert_eq!(site_of(&snapshot(key, kind)), "app", "{key}");
            assert_eq!(provider_of(kind), provider);
            assert!(is_per_site(kind));
        }
        let named = snapshot(super::super::named::NAMED_KEY, TunnelKind::Named);
        assert_eq!(site_of(&named), "@named");
        assert_eq!(provider_of(TunnelKind::Named), TunnelProvider::Cloudflare);
        assert!(!is_per_site(TunnelKind::Named));
        assert_eq!(site_of(&snapshot("app", TunnelKind::Zrok)), "app");
    }
}

#[cfg(all(test, unix))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod stub_tests {
    use super::super::supervise::tests::{poll_until, snapshots, stub_state, teardown, QUICK_STUB};
    use super::*;
    use yerd_ipc::{TunnelInfo, TunnelRunState};

    const TOKEN: &str = "2fXkStubToken0123456789abcdef";

    const NGROK_STUB: &[u8] = b"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOME/ngrok.args\"\nif [ -n \"$NGROK_AUTHTOKEN\" ]; then echo token-set > \"$HOME/ngrok.env\"; else echo token-missing > \"$HOME/ngrok.env\"; fi\necho \"proxy=$http_proxy\" >> \"$HOME/ngrok.env\"\nurl=https://stub.ngrok-free.app\nprev=\nfor a in \"$@\"; do if [ \"$prev\" = --url ]; then url=\"$a\"; fi; prev=\"$a\"; done\necho \"{\\\"lvl\\\":\\\"info\\\",\\\"msg\\\":\\\"started tunnel\\\",\\\"url\\\":\\\"$url\\\"}\"\nexec sleep 60\n";

    const NGROK_FAIL_STUB: &[u8] = b"#!/bin/sh\necho \"{\\\"lvl\\\":\\\"crit\\\",\\\"msg\\\":\\\"command failed\\\",\\\"err\\\":\\\"authentication failed: $NGROK_AUTHTOKEN rejected\\\"}\"\nexit 1\n";

    const ZROK_STUB: &[u8] = b"#!/bin/sh\necho \"$HOME\" > \"$HOME/zrok.home\"\necho '[   0.512]    INFO main: access your zrok share at the following endpoints:'\necho ' https://stubzrok.share.zrok.io'\nexec sleep 60\n";

    fn install(state: &DaemonState, tool: Tool, version: &str, stub: &[u8]) {
        tools::stage_and_swap(&state.dirs, tool, version, |staging| {
            tools::write_executable(staging, tool.primary_bin(), stub)
        })
        .unwrap();
    }

    async fn ngrok_state(tmp: &std::path::Path, stub: &[u8]) -> Arc<DaemonState> {
        let state = stub_state(tmp, QUICK_STUB).await;
        install(&state, Tool::Ngrok, "3.39.13", stub);
        state.config.lock().await.tunnel.ngrok.authtoken = Some(TOKEN.to_owned());
        state
    }

    async fn zrok_state(tmp: &std::path::Path) -> Arc<DaemonState> {
        let state = stub_state(tmp, QUICK_STUB).await;
        install(&state, Tool::Zrok, "v2.0.11", ZROK_STUB);
        state
    }

    fn enable_zrok(state: &DaemonState) {
        let dir = zrok_home(&state.dirs).join(".zrok2");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("environment.json"), "{}").unwrap();
    }

    fn tunnels(resp: &Response) -> &[TunnelInfo] {
        let Response::Tunnels { tunnels, .. } = resp else {
            panic!("expected Tunnels, got {resp:?}");
        };
        tunnels
    }

    fn share_of(resp: &Response, provider: TunnelProvider) -> TunnelInfo {
        tunnels(resp)
            .iter()
            .find(|t| t.site == "app" && t.provider == provider)
            .cloned()
            .unwrap_or_else(|| panic!("no {provider:?} share in {resp:?}"))
    }

    fn error_of(resp: &Response) -> (ErrorCode, String) {
        let Response::Error { code, message } = resp else {
            panic!("expected an error, got {resp:?}");
        };
        (*code, message.clone())
    }

    fn tunnel_file(state: &DaemonState, name: &str) -> String {
        std::fs::read_to_string(install::tunnel_dir(&state.dirs).join(name)).unwrap()
    }

    async fn routed(state: &DaemonState, host: &str) -> Option<String> {
        state
            .tunnel_routes
            .read()
            .await
            .get(host)
            .map(|r| r.site.clone())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_ngrok_older_than_the_minimum_is_refused_before_spawning() {
        let tmp = tempfile::tempdir().unwrap();
        let state = stub_state(tmp.path(), QUICK_STUB).await;
        install(&state, Tool::Ngrok, "3.15.0", NGROK_STUB);
        state.config.lock().await.tunnel.ngrok.authtoken = Some(TOKEN.to_owned());

        let resp = start_share_in("app", TunnelProvider::Ngrok, false, &state, None).await;

        let (code, message) = error_of(&resp);
        assert_eq!(code, ErrorCode::InvalidPath);
        assert!(message.contains("3.15.0 is too old"), "{message}");
        assert!(snapshots(&state).await.is_empty());
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_ngrok_random_share_runs_with_the_token_only_in_its_env() {
        let tmp = tempfile::tempdir().unwrap();
        let state = ngrok_state(tmp.path(), NGROK_STUB).await;

        let resp = start_share_in("app", TunnelProvider::Ngrok, false, &state, None).await;

        let info = share_of(&resp, TunnelProvider::Ngrok);
        assert_eq!(info.state, TunnelRunState::Running);
        assert!(!info.static_domain);
        assert_eq!(info.url.as_deref(), Some("https://stub.ngrok-free.app"));
        assert_eq!(
            routed(&state, "stub.ngrok-free.app").await.as_deref(),
            Some("app")
        );
        let args = tunnel_file(&state, "ngrok.args");
        assert!(!args.contains(TOKEN), "{args}");
        let config = install::tunnel_dir(&state.dirs).join("ngrok.yml");
        assert!(
            args.contains(&format!("--config\n{}\n", config.display())),
            "{args}"
        );
        assert!(!args.contains("--url"), "{args}");
        assert!(tunnel_file(&state, "ngrok.env").starts_with("token-set\n"));
        assert!(std::fs::read_to_string(&config)
            .unwrap()
            .contains("web_addr: false"));
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_ngrok_share_uses_the_static_domain_unless_random_is_asked() {
        let tmp = tempfile::tempdir().unwrap();
        let state = ngrok_state(tmp.path(), NGROK_STUB).await;
        state.config.lock().await.tunnel.ngrok.domain = Some("demo.ngrok-free.app".to_owned());

        let resp = start_share_in("app", TunnelProvider::Ngrok, false, &state, None).await;

        let info = share_of(&resp, TunnelProvider::Ngrok);
        assert!(info.static_domain);
        assert_eq!(info.url.as_deref(), Some("https://demo.ngrok-free.app"));
        assert!(tunnel_file(&state, "ngrok.args").contains("--url\nhttps://demo.ngrok-free.app\n"));

        super::super::stop_tunnel("app", Some(TunnelProvider::Ngrok), &state).await;
        let resp = start_share_in("app", TunnelProvider::Ngrok, true, &state, None).await;

        let info = share_of(&resp, TunnelProvider::Ngrok);
        assert!(!info.static_domain);
        assert_eq!(info.url.as_deref(), Some("https://stub.ngrok-free.app"));
        assert!(!tunnel_file(&state, "ngrok.args").contains("--url"));
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_second_static_domain_share_is_refused_but_a_random_one_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let state = ngrok_state(tmp.path(), NGROK_STUB).await;
        state.config.lock().await.tunnel.ngrok.domain = Some("demo.ngrok-free.app".to_owned());
        let root = tmp.path().join("blog");
        std::fs::create_dir_all(&root).unwrap();
        let blog =
            yerd_core::Site::linked("blog", &root, yerd_core::PhpVersion::new(8, 3)).unwrap();
        state.router.write().await.insert(blog).unwrap();
        start_share_in("app", TunnelProvider::Ngrok, false, &state, None).await;

        let resp = start_share_in("blog", TunnelProvider::Ngrok, false, &state, None).await;

        let (code, message) = error_of(&resp);
        assert_eq!(code, ErrorCode::AlreadyExists);
        assert!(message.contains("in use by app's share"), "{message}");
        assert!(!snapshots(&state)
            .await
            .iter()
            .any(|s| s.site == "blog@ngrok"));
        let resp = start_share_in("blog", TunnelProvider::Ngrok, true, &state, None).await;
        let info = tunnels(&resp)
            .iter()
            .find(|t| t.site == "blog" && t.provider == TunnelProvider::Ngrok)
            .cloned()
            .unwrap();
        assert!(!info.static_domain);
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_binaries_are_not_found_and_spawn_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let state = stub_state(tmp.path(), QUICK_STUB).await;
        enable_zrok(&state);
        state.config.lock().await.tunnel.ngrok.authtoken = Some(TOKEN.to_owned());

        for provider in [TunnelProvider::Ngrok, TunnelProvider::Zrok] {
            let resp = start_share_in("app", provider, false, &state, None).await;
            let (code, message) = error_of(&resp);
            assert_eq!(code, ErrorCode::NotFound);
            assert!(message.contains("Tooling"), "{message}");
        }
        assert!(snapshots(&state).await.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ngrok_without_a_token_is_refused_before_spawning() {
        let tmp = tempfile::tempdir().unwrap();
        let state = ngrok_state(tmp.path(), NGROK_STUB).await;
        state.config.lock().await.tunnel.ngrok.authtoken = None;

        let resp = start_share_in("app", TunnelProvider::Ngrok, false, &state, None).await;

        let (code, message) = error_of(&resp);
        assert_eq!(code, ErrorCode::InvalidPath);
        assert!(message.contains("yerd tunnel ngrok token"), "{message}");
        assert!(snapshots(&state).await.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_zrok_share_needs_the_enabled_environment_and_runs_in_it() {
        let tmp = tempfile::tempdir().unwrap();
        let state = zrok_state(tmp.path()).await;

        let resp = start_share_in("app", TunnelProvider::Zrok, false, &state, None).await;
        let (code, message) = error_of(&resp);
        assert_eq!(code, ErrorCode::InvalidPath);
        assert!(message.contains("yerd tunnel zrok enable"), "{message}");
        assert!(snapshots(&state).await.is_empty());

        enable_zrok(&state);
        let resp = start_share_in("app", TunnelProvider::Zrok, false, &state, None).await;

        let info = share_of(&resp, TunnelProvider::Zrok);
        assert_eq!(info.state, TunnelRunState::Running);
        assert_eq!(info.url.as_deref(), Some("https://stubzrok.share.zrok.io"));
        let home = zrok_home(&state.dirs);
        assert_eq!(
            std::fs::read_to_string(home.join("zrok.home"))
                .unwrap()
                .trim(),
            home.display().to_string()
        );
        assert_eq!(
            routed(&state, "stubzrok.share.zrok.io").await.as_deref(),
            Some("app")
        );
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_site_holds_one_share_per_provider_and_stops_them_independently() {
        let tmp = tempfile::tempdir().unwrap();
        let state = ngrok_state(tmp.path(), NGROK_STUB).await;

        let quick = start_share_in("app", TunnelProvider::Cloudflare, false, &state, None).await;
        assert_eq!(
            share_of(&quick, TunnelProvider::Cloudflare).state,
            TunnelRunState::Running
        );
        let both = start_share_in("app", TunnelProvider::Ngrok, false, &state, None).await;
        assert_eq!(tunnels(&both).len(), 2);
        assert_eq!(super::super::shared_site_count(&state).await, 1);

        let left = super::super::stop_tunnel("app", Some(TunnelProvider::Ngrok), &state).await;

        assert_eq!(tunnels(&left).len(), 1);
        share_of(&left, TunnelProvider::Cloudflare);
        assert_eq!(
            routed(&state, "stub-tunnel.trycloudflare.com")
                .await
                .as_deref(),
            Some("app")
        );
        assert_eq!(routed(&state, "stub.ngrok-free.app").await, None);

        start_share_in("app", TunnelProvider::Ngrok, false, &state, None).await;
        let none = super::super::stop_tunnel("app", None, &state).await;

        assert!(tunnels(&none).is_empty());
        assert!(snapshots(&state).await.is_empty());
        assert!(state.tunnel_routes.read().await.is_empty());
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failing_ngrok_errors_without_leaking_the_token() {
        let tmp = tempfile::tempdir().unwrap();
        let state = ngrok_state(tmp.path(), NGROK_FAIL_STUB).await;

        let resp = start_share_in("app", TunnelProvider::Ngrok, false, &state, None).await;

        let (_, message) = error_of(&resp);
        assert!(message.contains("***"), "{message}");
        assert!(!message.contains(TOKEN), "{message}");
        assert!(snapshots(&state).await.is_empty());
        let log = super::super::logfile(&state.dirs, "app@ngrok");
        let text = std::fs::read_to_string(log).unwrap_or_default();
        assert!(!text.contains(TOKEN), "{text}");
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_ngrok_share_gets_the_outbound_proxy_env() {
        use yerd_core::network::{OutboundProxyMode, ProxyUrl};
        let tmp = tempfile::tempdir().unwrap();
        let state = ngrok_state(tmp.path(), NGROK_STUB).await;
        {
            let mut c = state.config.lock().await;
            c.network.mode = OutboundProxyMode::Manual;
            c.network.http_proxy = Some(ProxyUrl::parse("http://proxy.corp:3128").unwrap());
        }
        state
            .network
            .refresh(&state, crate::network::LoginShell::UseCached)
            .await;

        let resp = start_share_in("app", TunnelProvider::Ngrok, false, &state, None).await;

        share_of(&resp, TunnelProvider::Ngrok);
        let env = tunnel_file(&state, "ngrok.env");
        assert!(env.contains("proxy=http://proxy.corp:3128"), "{env}");
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn provider_uninstalls_are_guarded_by_their_own_shares() {
        let tmp = tempfile::tempdir().unwrap();
        let state = ngrok_state(tmp.path(), NGROK_STUB).await;
        start_share_in("app", TunnelProvider::Ngrok, false, &state, None).await;

        let (code, message) = error_of(&crate::ipc_server::uninstall_tool("ngrok", &state).await);
        assert_eq!(code, ErrorCode::InvalidPath);
        assert!(message.contains("ngrok share"), "{message}");
        assert_eq!(
            crate::ipc_server::uninstall_tool("cloudflared", &state).await,
            Response::Ok
        );

        super::super::stop_tunnel("app", None, &state).await;
        assert_eq!(
            crate::ipc_server::uninstall_tool("ngrok", &state).await,
            Response::Ok
        );
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disabling_zrok_is_refused_while_a_zrok_share_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let state = zrok_state(tmp.path()).await;
        enable_zrok(&state);
        start_share_in("app", TunnelProvider::Zrok, false, &state, None).await;

        let resp = super::super::providers::disable_zrok_in(&state, None).await;

        let (code, message) = error_of(&resp);
        assert_eq!(code, ErrorCode::InvalidPath);
        assert!(message.contains("stop every zrok share"), "{message}");
        assert!(zrok_home(&state.dirs)
            .join(".zrok2/environment.json")
            .is_file());
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_share_without_a_provider_uses_the_configured_default() {
        let tmp = tempfile::tempdir().unwrap();
        let state = zrok_state(tmp.path()).await;
        enable_zrok(&state);
        state.config.lock().await.tunnel.default_provider = TunnelProvider::Zrok;

        let provider = chosen_provider(None, &state).await;
        assert_eq!(provider, TunnelProvider::Zrok);
        assert_eq!(
            chosen_provider(Some(TunnelProvider::Ngrok), &state).await,
            TunnelProvider::Ngrok
        );
        let resp = start_share_in("app", provider, false, &state, None).await;

        assert_eq!(
            share_of(&resp, TunnelProvider::Zrok).state,
            TunnelRunState::Running
        );
        poll_until(&state, |s| s.iter().any(|t| t.site == "app@zrok")).await;
        teardown(&state).await;
    }
}
