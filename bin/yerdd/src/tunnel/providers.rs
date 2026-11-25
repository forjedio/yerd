//! Tunnel provider configuration: per-provider readiness for the Providers
//! sheet and `yerd tunnel providers`, the ngrok authtoken and static domain,
//! the default provider for new shares, and the zrok environment.
//!
//! zrok always runs in Yerd's own environment: `HOME` is pinned to
//! `{data}/tunnel/zrok`, so zrok keeps its state in `{data}/tunnel/zrok/.zrok2`
//! and never touches the user's `~/.zrok2`. Secrets (the ngrok authtoken, the
//! zrok account token) are never logged, echoed in errors, or returned
//! unmasked.
//!
//! Every function that would search the user's login-shell `PATH` has an `_in`
//! variant taking that `PATH` as an argument, so tests never find or run a
//! real `ngrok` or `zrok2`.

use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;

use yerd_config::Config;
use yerd_core::tunnel_provider::{
    is_valid_provider_token, mask_secret, normalize_static_domain, readiness, TunnelProvider,
};
use yerd_ipc::{ErrorCode, Response, TunnelProviderStatus};
use yerd_platform::PlatformDirs;
use yerd_tunnel::providers::{
    display_version, failure_line, parse_version_token, version_args, zrok_disable_args,
    zrok_enable_args,
};

use super::install::{self, CloudflaredSource};
use crate::state::DaemonState;
use crate::tools::{self, Tool};

/// Bound on the `<bin> version` probe for a binary found on `PATH`.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bound on a `zrok2 enable`/`disable` one-shot, which talks to the zrok API.
const ZROK_TIMEOUT: Duration = Duration::from_secs(60);

const ZROK_NOT_INSTALLED: &str =
    "zrok is not installed - install it from Tooling or with `yerd install tool zrok`";

/// A resolved ngrok or zrok binary.
#[derive(Debug, Clone)]
pub(crate) struct ProviderBinary {
    /// The binary to spawn.
    pub path: PathBuf,
    /// Whether it is Yerd's managed copy or one found on the user's `PATH`.
    pub source: CloudflaredSource,
    /// Its version without a leading `v`, when known.
    pub version: Option<String>,
}

/// `tool`'s managed binary path, whether or not it is installed.
pub(crate) fn managed_binary(dirs: &PlatformDirs, tool: Tool) -> PathBuf {
    tools::tool_dir(dirs, tool).join(tool.primary_bin())
}

/// Resolve `tool`'s binary: Yerd's managed copy when installed, else (only
/// when `user_path` is given) a copy on that `PATH`, probed for its version.
pub(crate) async fn resolve_binary_in(
    dirs: &PlatformDirs,
    tool: Tool,
    user_path: Option<&[PathBuf]>,
) -> Option<ProviderBinary> {
    let managed = managed_binary(dirs, tool);
    if managed.is_file() {
        return Some(ProviderBinary {
            path: managed,
            source: CloudflaredSource::Managed,
            version: tools::installed_version(dirs, tool, None)
                .map(|v| display_version(&v).to_owned()),
        });
    }
    let path = tools::external::external_tool(user_path?, tool, &tools::bin_dir(dirs), &dirs.data)?;
    let version = probe_version(dirs, &path).await;
    Some(ProviderBinary {
        path,
        source: CloudflaredSource::System,
        version,
    })
}

async fn probe_version(dirs: &PlatformDirs, path: &Path) -> Option<String> {
    probe_version_within(dirs, path, VERSION_PROBE_TIMEOUT).await
}

/// Runs `<path> version` with a cleared environment (the forwarded allowlist
/// plus `HOME={data}/tunnel`, never the zrok environment) and parses the
/// version from stdout.
async fn probe_version_within(dirs: &PlatformDirs, path: &Path, limit: Duration) -> Option<String> {
    let mut cmd = tokio::process::Command::new(path);
    cmd.args(version_args())
        .env_clear()
        .envs(super::forwarded_env())
        .env("HOME", install::tunnel_dir(dirs))
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(limit, cmd.output()).await.ok()?.ok()?;
    parse_version_token(&String::from_utf8_lossy(&out.stdout))
}

/// `{data}/tunnel/zrok`, the `HOME` every zrok command and share runs with.
pub(crate) fn zrok_home(dirs: &PlatformDirs) -> PathBuf {
    install::tunnel_dir(dirs).join("zrok")
}

/// Whether Yerd's zrok environment is enabled (zrok wrote its
/// `environment.json`).
pub(crate) fn zrok_enabled(dirs: &PlatformDirs) -> bool {
    zrok_home(dirs)
        .join(".zrok2")
        .join("environment.json")
        .is_file()
}

pub(crate) fn tool_for(provider: TunnelProvider) -> Option<Tool> {
    match provider {
        TunnelProvider::Cloudflare => None,
        TunnelProvider::Ngrok => Some(Tool::Ngrok),
        TunnelProvider::Zrok => Some(Tool::Zrok),
    }
}

async fn status_in(
    state: &DaemonState,
    provider: TunnelProvider,
    user_path: Option<&[PathBuf]>,
) -> TunnelProviderStatus {
    let Some(tool) = tool_for(provider) else {
        let cloudflared = super::cloudflared_status(state).await;
        return TunnelProviderStatus {
            provider,
            readiness: readiness(provider, cloudflared.installed, cloudflared.logged_in),
            version: cloudflared.version,
            source: cloudflared.source,
            path: None,
            update_available: cloudflared.update_available,
            configured: cloudflared.logged_in,
            authtoken_hint: None,
            domain: None,
        };
    };
    let binary = resolve_binary_in(&state.dirs, tool, user_path).await;
    let update_available = match binary.as_ref() {
        Some(ProviderBinary {
            source: CloudflaredSource::Managed,
            version: Some(version),
            ..
        }) => crate::tool_updates::cached(state, tool.id())
            .await
            .and_then(|latest| tools::newer_version(version, &latest))
            .map(|v| display_version(&v).to_owned()),
        _ => None,
    };
    let (configured, authtoken_hint, domain) = if provider == TunnelProvider::Ngrok {
        let cfg = state.config.lock().await;
        let ngrok = &cfg.tunnel.ngrok;
        (
            ngrok.authtoken.is_some(),
            ngrok.authtoken.as_deref().map(mask_secret),
            ngrok.domain.clone(),
        )
    } else {
        (zrok_enabled(&state.dirs), None, None)
    };
    TunnelProviderStatus {
        provider,
        readiness: readiness(provider, binary.is_some(), configured),
        version: binary.as_ref().and_then(|b| b.version.clone()),
        source: binary.as_ref().map(|b| super::source_to_wire(b.source)),
        path: binary
            .as_ref()
            .filter(|b| b.source == CloudflaredSource::System)
            .map(|b| b.path.display().to_string()),
        update_available,
        configured,
        authtoken_hint,
        domain,
    }
}

/// [`list`] against an explicit user `PATH`.
pub(crate) async fn list_in(state: &DaemonState, user_path: Option<&[PathBuf]>) -> Response {
    let mut providers = Vec::with_capacity(TunnelProvider::ALL.len());
    for provider in TunnelProvider::ALL {
        providers.push(status_in(state, provider, user_path).await);
    }
    let default_provider = state.config.lock().await.tunnel.default_provider;
    Response::TunnelProviders {
        providers,
        default_provider,
    }
}

/// Every provider's readiness and settings plus the default for new shares.
/// Replies [`Response::TunnelProviders`].
pub async fn list(state: &DaemonState) -> Response {
    let user_path = tools::external::resolve_user_path().await;
    list_in(state, user_path.as_deref()).await
}

fn error(code: ErrorCode, message: impl Into<String>) -> Response {
    Response::Error {
        code,
        message: message.into(),
    }
}

/// Clone the config, apply `mutate`, validate, save, then swap it in. A
/// validation failure replies `InvalidPath` with `what` (the reason never
/// carries the value).
async fn save_config(
    state: &DaemonState,
    what: &str,
    mutate: impl FnOnce(&mut Config),
) -> Result<(), Response> {
    let mut guard = state.config.lock().await;
    let mut new = guard.clone();
    mutate(&mut new);
    if let Err(e) = new.validate() {
        return Err(error(ErrorCode::InvalidPath, format!("{what}: {e}")));
    }
    if let Err(e) = new.save(&state.config_path) {
        return Err(error(
            ErrorCode::Internal,
            format!("config save failed: {e}"),
        ));
    }
    *guard = new;
    Ok(())
}

/// Save (trimmed) or clear the ngrok authtoken. Replies [`Response::Ok`].
pub async fn set_ngrok_authtoken(state: &DaemonState, authtoken: Option<&str>) -> Response {
    let token = authtoken.map(|t| t.trim().to_owned());
    let cleared = token.is_none();
    if let Err(resp) = save_config(state, "invalid ngrok authtoken", |c| {
        c.tunnel.ngrok.authtoken = token;
    })
    .await
    {
        return resp;
    }
    tracing::info!(cleared, "saved ngrok authtoken");
    Response::Ok
}

/// Save or clear the ngrok static domain. The input is normalised to a bare
/// hostname, and a value that is empty after normalising clears it. Replies
/// [`Response::Ok`].
pub async fn set_ngrok_domain(state: &DaemonState, domain: Option<&str>) -> Response {
    let domain = domain
        .map(normalize_static_domain)
        .filter(|d| !d.is_empty());
    let cleared = domain.is_none();
    if let Err(resp) = save_config(state, "invalid static domain", |c| {
        c.tunnel.ngrok.domain = domain;
    })
    .await
    {
        return resp;
    }
    tracing::info!(cleared, "saved ngrok static domain");
    Response::Ok
}

/// Set the default provider for new shares. Replies [`Response::Ok`].
pub async fn set_default(state: &DaemonState, provider: TunnelProvider) -> Response {
    if let Err(resp) = save_config(state, "invalid tunnel config", |c| {
        c.tunnel.default_provider = provider;
    })
    .await
    {
        return resp;
    }
    tracing::info!(provider = %provider.as_str(), "saved default tunnel provider");
    Response::Ok
}

/// Enable Yerd's zrok environment with an account token. Replies
/// [`Response::Ok`].
pub async fn enable_zrok(state: &DaemonState, token: &str) -> Response {
    let user_path = tools::external::resolve_user_path().await;
    enable_zrok_in(state, token, user_path.as_deref()).await
}

/// [`enable_zrok`] against an explicit user `PATH`.
pub(crate) async fn enable_zrok_in(
    state: &DaemonState,
    token: &str,
    user_path: Option<&[PathBuf]>,
) -> Response {
    let token = token.trim();
    if !is_valid_provider_token(token) {
        return error(ErrorCode::InvalidPath, "invalid zrok account token");
    }
    let Some(binary) = resolve_binary_in(&state.dirs, Tool::Zrok, user_path).await else {
        return error(ErrorCode::NotFound, ZROK_NOT_INSTALLED);
    };
    let _guard = state.zrok_mutate.lock().await;
    if zrok_enabled(&state.dirs) {
        return error(
            ErrorCode::AlreadyExists,
            "zrok is already enabled on this machine - disable it first",
        );
    }
    if let Err(e) = crate::secure_fs::create_private_dir(&zrok_home(&state.dirs)) {
        return error(
            ErrorCode::Internal,
            format!("could not prepare the zrok dir: {e}"),
        );
    }
    match run_zrok(state, &binary.path, zrok_enable_args(token)).await {
        Err(e) => error(ErrorCode::Internal, format!("zrok enable failed: {e}")),
        Ok(out) if !out.status.success() => error(
            ErrorCode::Internal,
            format!("zrok enable failed: {}", output_failure(&out, token)),
        ),
        Ok(_) if !zrok_enabled(&state.dirs) => error(
            ErrorCode::Internal,
            "zrok enable did not write an environment",
        ),
        Ok(_) => {
            tracing::info!("enabled zrok environment");
            Response::Ok
        }
    }
}

/// Disable Yerd's zrok environment, releasing this machine from the zrok
/// account. Replies [`Response::Ok`].
pub async fn disable_zrok(state: &DaemonState) -> Response {
    let user_path = tools::external::resolve_user_path().await;
    disable_zrok_in(state, user_path.as_deref()).await
}

/// [`disable_zrok`] against an explicit user `PATH`.
///
/// When zrok refuses (for example a revoked account token) it keeps the local
/// environment, so the reported state stays truthful.
pub(crate) async fn disable_zrok_in(
    state: &DaemonState,
    user_path: Option<&[PathBuf]>,
) -> Response {
    let Some(binary) = resolve_binary_in(&state.dirs, Tool::Zrok, user_path).await else {
        return error(ErrorCode::NotFound, ZROK_NOT_INSTALLED);
    };
    let _guard = state.zrok_mutate.lock().await;
    if zrok_share_running(state).await {
        return error(
            ErrorCode::InvalidPath,
            "stop every zrok share before disabling zrok",
        );
    }
    if !zrok_enabled(&state.dirs) {
        return error(ErrorCode::NotFound, "zrok is not enabled");
    }
    match run_zrok(state, &binary.path, zrok_disable_args()).await {
        Err(e) => error(ErrorCode::Internal, format!("zrok disable failed: {e}")),
        Ok(out) if !out.status.success() => error(
            ErrorCode::Internal,
            format!("zrok disable failed: {}", output_failure(&out, "")),
        ),
        Ok(_) => {
            tracing::info!("disabled zrok environment");
            Response::Ok
        }
    }
}

/// Whether any zrok share instance exists, in any state. Taken under
/// `zrok_mutate` (lock order `zrok_mutate`, then `tunnel_manager`, as a zrok
/// share start), so no share can start between this check and a disable.
async fn zrok_share_running(state: &DaemonState) -> bool {
    state
        .tunnel_manager
        .lock()
        .await
        .snapshots()
        .iter()
        .any(|s| matches!(s.kind, yerd_tunnel::TunnelKind::Zrok))
}

fn output_failure(out: &Output, secret: &str) -> String {
    failure_line(
        &String::from_utf8_lossy(&out.stderr),
        &String::from_utf8_lossy(&out.stdout),
        secret,
    )
}

/// Run one zrok command in Yerd's zrok environment: cleared env plus the
/// forwarded allowlist, `HOME={data}/tunnel/zrok`, and the outbound proxy
/// env. Errors never include the argv or output.
async fn run_zrok(
    state: &DaemonState,
    binary: &Path,
    args: Vec<std::ffi::OsString>,
) -> Result<Output, String> {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.args(args)
        .env_clear()
        .envs(super::forwarded_env())
        .env("HOME", zrok_home(&state.dirs))
        .stdin(Stdio::null())
        .kill_on_drop(true);
    crate::network::apply_child_env(&mut cmd, state).await;
    match tokio::time::timeout(ZROK_TIMEOUT, cmd.output()).await {
        Err(_) => Err("timed out".to_owned()),
        Ok(Err(e)) => Err(format!("could not run zrok2: {}", e.kind())),
        Ok(Ok(out)) => Ok(out),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::test_support::state_in;
    use yerd_core::tunnel_provider::ProviderReadiness;

    const TOKEN: &str = "2fXkAbCdEfGhIjKlMnOpQrStUvWxYz0123456789abcdQa";

    async fn seed_cloudflared(state: &DaemonState) {
        *state.cloudflared_resolution.write().await = Some(install::Resolved {
            binary: state.dirs.data.join("cloudflared"),
            source: CloudflaredSource::Managed,
            version: Some("2026.6.1".into()),
        });
    }

    fn install_managed(state: &DaemonState, tool: Tool, version: &str, script: &str) {
        tools::stage_and_swap(&state.dirs, tool, version, |staging| {
            tools::write_executable(staging, tool.primary_bin(), script.as_bytes())
        })
        .unwrap();
    }

    async fn providers(state: &DaemonState) -> (Vec<TunnelProviderStatus>, TunnelProvider) {
        seed_cloudflared(state).await;
        match list_in(state, None).await {
            Response::TunnelProviders {
                providers,
                default_provider,
            } => (providers, default_provider),
            other => panic!("unexpected {other:?}"),
        }
    }

    async fn status_of(state: &DaemonState, provider: TunnelProvider) -> TunnelProviderStatus {
        providers(state)
            .await
            .0
            .into_iter()
            .find(|s| s.provider == provider)
            .unwrap()
    }

    fn write_env_json(state: &DaemonState) {
        let dir = zrok_home(&state.dirs).join(".zrok2");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("environment.json"), "{}").unwrap();
    }

    #[cfg(unix)]
    fn fake_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[tokio::test]
    async fn no_binaries_reports_ngrok_and_zrok_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let (list, default_provider) = providers(&state).await;
        let order: Vec<TunnelProvider> = list.iter().map(|s| s.provider).collect();
        assert_eq!(order, TunnelProvider::ALL);
        assert_eq!(default_provider, TunnelProvider::Cloudflare);
        for status in list
            .iter()
            .filter(|s| s.provider != TunnelProvider::Cloudflare)
        {
            assert_eq!(status.readiness, ProviderReadiness::NotInstalled);
            assert_eq!(status.source, None);
            assert!(!status.configured);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn path_copy_reports_system_source_path_and_probed_version() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let bin = tmp.path().join("userbin");
        std::fs::create_dir_all(&bin).unwrap();
        let path = fake_script(&bin, "ngrok", "#!/bin/sh\necho 'ngrok version 3.18.4'\n");
        let user_path = vec![bin];
        let status = status_in(&state, TunnelProvider::Ngrok, Some(&user_path)).await;
        assert_eq!(status.source, Some(yerd_ipc::CloudflaredSource::System));
        assert_eq!(status.path.as_deref(), Some(path.to_str().unwrap()));
        assert_eq!(status.version.as_deref(), Some("3.18.4"));
        assert_eq!(status.readiness, ProviderReadiness::NeedsSetup);
        assert_eq!(status.update_available, None);
    }

    #[tokio::test]
    async fn managed_ngrok_with_token_is_ready_and_masked() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        install_managed(&state, Tool::Ngrok, "3.39.13", "#!/bin/sh\nexit 0\n");
        assert_eq!(set_ngrok_authtoken(&state, Some(TOKEN)).await, Response::Ok);
        seed_cloudflared(&state).await;
        let resp = list_in(&state, None).await;
        assert!(!serde_json::to_string(&resp).unwrap().contains(TOKEN));
        let status = status_of(&state, TunnelProvider::Ngrok).await;
        assert_eq!(status.readiness, ProviderReadiness::Ready);
        assert_eq!(status.source, Some(yerd_ipc::CloudflaredSource::Managed));
        assert_eq!(status.version.as_deref(), Some("3.39.13"));
        assert_eq!(status.path, None);
        assert_eq!(status.authtoken_hint.as_deref(), Some("2fXk••••••••Qa"));
    }

    #[tokio::test]
    async fn managed_zrok_reports_bare_version_and_enabled_state() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        install_managed(&state, Tool::Zrok, "v2.0.11", "#!/bin/sh\nexit 0\n");
        let status = status_of(&state, TunnelProvider::Zrok).await;
        assert_eq!(status.version.as_deref(), Some("2.0.11"));
        assert_eq!(status.readiness, ProviderReadiness::NeedsSetup);
        write_env_json(&state);
        let status = status_of(&state, TunnelProvider::Zrok).await;
        assert_eq!(status.readiness, ProviderReadiness::Ready);
        assert!(status.configured);
    }

    #[tokio::test]
    async fn managed_copy_with_cached_newer_release_reports_update() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        install_managed(&state, Tool::Zrok, "v2.0.11", "#!/bin/sh\nexit 0\n");
        install_managed(&state, Tool::Ngrok, "3.39.13", "#!/bin/sh\nexit 0\n");
        {
            let mut cache = state.tool_updates.write().await;
            cache.insert(Tool::Zrok.id().to_owned(), "v2.0.12".to_owned());
            cache.insert(Tool::Ngrok.id().to_owned(), "3.40.0".to_owned());
        }
        let zrok = status_of(&state, TunnelProvider::Zrok).await;
        assert_eq!(zrok.update_available.as_deref(), Some("2.0.12"));
        let ngrok = status_of(&state, TunnelProvider::Ngrok).await;
        assert_eq!(ngrok.update_available.as_deref(), Some("3.40.0"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_version_parses_and_times_out() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let ok = fake_script(
            tmp.path(),
            "ngrok-ok",
            "#!/bin/sh\necho 'ngrok version 3.18.4'\n",
        );
        assert_eq!(
            probe_version(&state.dirs, &ok).await.as_deref(),
            Some("3.18.4")
        );
        let slow = fake_script(tmp.path(), "ngrok-slow", "#!/bin/sh\nexec sleep 30\n");
        assert_eq!(
            probe_version_within(&state.dirs, &slow, Duration::from_millis(200)).await,
            None
        );
    }

    #[tokio::test]
    async fn ngrok_authtoken_is_trimmed_saved_rejected_and_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert_eq!(
            set_ngrok_authtoken(&state, Some("  tok_123  ")).await,
            Response::Ok
        );
        let on_disk = Config::load(&state.config_path).unwrap();
        assert_eq!(on_disk.tunnel.ngrok.authtoken.as_deref(), Some("tok_123"));

        let resp = set_ngrok_authtoken(&state, Some("a b")).await;
        let Response::Error { code, message } = resp else {
            panic!("expected an error");
        };
        assert_eq!(code, ErrorCode::InvalidPath);
        assert!(message.starts_with("invalid ngrok authtoken"), "{message}");
        assert!(!message.contains("a b"), "{message}");
        let on_disk = Config::load(&state.config_path).unwrap();
        assert_eq!(on_disk.tunnel.ngrok.authtoken.as_deref(), Some("tok_123"));

        assert_eq!(set_ngrok_authtoken(&state, None).await, Response::Ok);
        let on_disk = Config::load(&state.config_path).unwrap();
        assert_eq!(on_disk.tunnel.ngrok.authtoken, None);
    }

    #[tokio::test]
    async fn ngrok_domain_is_normalised_rejected_and_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert_eq!(
            set_ngrok_domain(&state, Some("https://X.ngrok-free.app/")).await,
            Response::Ok
        );
        let on_disk = Config::load(&state.config_path).unwrap();
        assert_eq!(
            on_disk.tunnel.ngrok.domain.as_deref(),
            Some("x.ngrok-free.app")
        );
        assert!(matches!(
            set_ngrok_domain(&state, Some("nodot")).await,
            Response::Error {
                code: ErrorCode::InvalidPath,
                ..
            }
        ));
        assert_eq!(set_ngrok_domain(&state, Some("  ")).await, Response::Ok);
        let on_disk = Config::load(&state.config_path).unwrap();
        assert_eq!(on_disk.tunnel.ngrok.domain, None);
    }

    #[tokio::test]
    async fn default_provider_round_trips_through_list() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert_eq!(
            set_default(&state, TunnelProvider::Zrok).await,
            Response::Ok
        );
        assert_eq!(providers(&state).await.1, TunnelProvider::Zrok);
        let on_disk = Config::load(&state.config_path).unwrap();
        assert_eq!(on_disk.tunnel.default_provider, TunnelProvider::Zrok);
    }

    #[cfg(unix)]
    fn recording_zrok(log: &Path) -> String {
        format!(
            "#!/bin/sh\n\
             echo \"HOME=$HOME ARGS=$1 $2 $3 PROXY=$http_proxy\" >> '{}'\n\
             if [ \"$1\" = enable ]; then mkdir -p \"$HOME/.zrok2\"; echo '{{}}' > \"$HOME/.zrok2/environment.json\"; fi\n\
             if [ \"$1\" = disable ]; then rm -f \"$HOME/.zrok2/environment.json\"; fi\n\
             exit 0\n",
            log.display()
        )
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn zrok_enable_and_disable_run_in_the_daemon_environment() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let log = tmp.path().join("zrok.log");
        install_managed(&state, Tool::Zrok, "v2.0.11", &recording_zrok(&log));

        assert_eq!(enable_zrok_in(&state, TOKEN, None).await, Response::Ok);
        let recorded = std::fs::read_to_string(&log).unwrap();
        let home = zrok_home(&state.dirs);
        assert!(
            recorded.contains(&format!("HOME={} ", home.display())),
            "{recorded}"
        );
        assert!(
            recorded.contains(&format!("ARGS=enable --headless {TOKEN} ")),
            "{recorded}"
        );
        let mode = std::fs::metadata(&home).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        assert_eq!(
            status_of(&state, TunnelProvider::Zrok).await.readiness,
            ProviderReadiness::Ready
        );

        assert!(matches!(
            enable_zrok_in(&state, TOKEN, None).await,
            Response::Error {
                code: ErrorCode::AlreadyExists,
                ..
            }
        ));
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);

        assert_eq!(disable_zrok_in(&state, None).await, Response::Ok);
        assert_eq!(
            status_of(&state, TunnelProvider::Zrok).await.readiness,
            ProviderReadiness::NeedsSetup
        );
        assert!(matches!(
            disable_zrok_in(&state, None).await,
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn zrok_failure_redacts_the_token() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        install_managed(
            &state,
            Tool::Zrok,
            "v2.0.11",
            "#!/bin/sh\necho \"[ERROR]: unable to enable with token $3\" >&2\nexit 1\n",
        );
        let Response::Error { code, message } = enable_zrok_in(&state, TOKEN, None).await else {
            panic!("expected an error");
        };
        assert_eq!(code, ErrorCode::Internal);
        assert!(message.contains("***"), "{message}");
        assert!(!message.contains(TOKEN), "{message}");
        assert!(!zrok_enabled(&state.dirs));
    }

    #[tokio::test]
    async fn zrok_rejects_bad_tokens_and_missing_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        for token in ["-x", "a b", ""] {
            assert!(
                matches!(
                    enable_zrok_in(&state, token, None).await,
                    Response::Error {
                        code: ErrorCode::InvalidPath,
                        ..
                    }
                ),
                "{token:?}"
            );
        }
        assert!(matches!(
            enable_zrok_in(&state, TOKEN, None).await,
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));
        assert!(matches!(
            disable_zrok_in(&state, None).await,
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn zrok_gets_the_outbound_proxy_env() {
        use yerd_core::network::{OutboundProxyMode, ProxyUrl};
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let log = tmp.path().join("zrok.log");
        install_managed(&state, Tool::Zrok, "v2.0.11", &recording_zrok(&log));
        {
            let mut c = state.config.lock().await;
            c.network.mode = OutboundProxyMode::Manual;
            c.network.http_proxy = Some(ProxyUrl::parse("http://proxy.corp:3128").unwrap());
        }
        state
            .network
            .refresh(&state, crate::network::LoginShell::UseCached)
            .await;
        assert_eq!(enable_zrok_in(&state, TOKEN, None).await, Response::Ok);
        let recorded = std::fs::read_to_string(&log).unwrap();
        assert!(
            recorded.contains("PROXY=http://proxy.corp:3128"),
            "{recorded}"
        );
    }
}
