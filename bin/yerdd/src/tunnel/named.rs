//! Named Tunnel (Phase 2) handlers: Cloudflare account login, tunnel
//! create/list/route, per-site hostname persistence, and named-tunnel start.
//!
//! Account-mutating `cloudflared` subcommands (login/create/route) run as
//! one-shot children with `HOME` and the origin cert pinned under
//! `{data}/tunnel`, so `cloudflared` never touches `~/.cloudflared` and every
//! secret stays in the daemon-owned 0700 dir. The long-running `tunnel run` is
//! launched through [`super::supervise::launch`] and supervised in the
//! background like a quick tunnel.
//!
//! Model (v1): one Cloudflare login and a single named tunnel (creating a second
//! is rejected), plus a per-site `site → hostname` mapping. `StartNamedTunnel`
//! renders one `named.yml` whose single catch-all ingress rule points at the
//! daemon's tunnel listener, which routes each public hostname to its site, so
//! the one tunnel serves every enabled site.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};

use yerd_ipc::{ErrorCode, NamedTunnelMeta, Response, SiteHostname};
use yerd_platform::PlatformDirs;
use yerd_tunnel::parse::{find_auth_url, find_tunnel_id};
use yerd_tunnel::TunnelKind;

use super::install;
use crate::state::DaemonState;

/// Bound on the network one-shots (`create` / `route dns`).
const ONESHOT_TIMEOUT: Duration = Duration::from_secs(60);

/// Bound on the interactive browser login. It holds `tunnel_mutate` while it
/// waits, so it must not wait forever if the user never finishes the browser
/// flow; five minutes is generous for a real login.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// Reserved supervisor key for the single consolidated named-tunnel process.
/// The `@` can't appear in a DNS label, so it never collides with a real site.
pub(super) const NAMED_KEY: &str = "@named";

/// How the interactive `cloudflared tunnel login` child ended.
enum LoginEnd {
    /// The child exited on its own (success or failure determined by the cert).
    Exited,
    /// The client cancelled the job.
    Cancelled,
    /// The browser authorization was not completed within [`LOGIN_TIMEOUT`].
    TimedOut,
}

/// `{data}/tunnel/.cloudflared/cert.pem` - the account origin cert.
///
/// `cloudflared tunnel login` ignores `--origincert` for the *write* and always
/// saves the cert to `$HOME/.cloudflared/cert.pem`; since we pin
/// `HOME={data}/tunnel`, that resolves here. Reading commands (`create`/`route`/
/// `run`) are pointed back at this same path via `--origincert`.
pub(super) fn origincert(dirs: &PlatformDirs) -> PathBuf {
    install::tunnel_dir(dirs)
        .join(".cloudflared")
        .join("cert.pem")
}

/// `{data}/tunnel/creds` - per-tunnel credentials JSONs.
fn creds_dir(dirs: &PlatformDirs) -> PathBuf {
    install::tunnel_dir(dirs).join("creds")
}

/// `{data}/tunnel/creds/<name>.json`.
fn creds_file(dirs: &PlatformDirs, name: &str) -> PathBuf {
    creds_dir(dirs).join(format!("{name}.json"))
}

/// Whether a tunnel name is safe to use as a path component and a `cloudflared`
/// argument: non-empty, bounded, limited to DNS-label-ish characters so it can
/// never escape `creds/` (no `/`, `..`, NUL, or whitespace), and not starting
/// with `-` (which `cloudflared` could misparse as a flag).
fn is_valid_tunnel_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        && name != "."
        && name != ".."
}

/// A conservative guard for a value passed as a positional `cloudflared`
/// argument (e.g. a routed hostname): non-empty, bounded, no leading `-`, and
/// only DNS-name characters, so it can't be misparsed as a flag.
fn is_safe_cli_value(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_')
}

/// Whether a Cloudflare account is logged in (the origin cert is present).
#[must_use]
pub fn is_logged_in(dirs: &PlatformDirs) -> bool {
    origincert(dirs).is_file()
}

/// Create the daemon-owned secret dirs (`{data}/tunnel`, `creds`) as 0700.
fn ensure_secret_dirs(dirs: &PlatformDirs) -> Result<(), String> {
    crate::secure_fs::create_private_dir(&install::tunnel_dir(dirs))
        .and_then(|()| crate::secure_fs::create_private_dir(&creds_dir(dirs)))
        .map_err(|e| format!("could not prepare tunnel dir: {e}"))
}

fn not_installed() -> Response {
    Response::Error {
        code: ErrorCode::NotFound,
        message: "cloudflared is not installed - install it from Integrations first".into(),
    }
}

fn need_login() -> Response {
    Response::Error {
        code: ErrorCode::NotFound,
        message: "not logged in to Cloudflare - run the account login first".into(),
    }
}

fn internal(message: String) -> Response {
    Response::Error {
        code: ErrorCode::Internal,
        message,
    }
}

/// Run a `cloudflared` subcommand to completion with `HOME`/origin-cert pinned,
/// capturing its output. Bounded by [`ONESHOT_TIMEOUT`]. `binary` is the
/// already-resolved `cloudflared` to spawn (see `super::resolved_cloudflared`).
async fn run_oneshot(
    dirs: &PlatformDirs,
    binary: &Path,
    args: Vec<OsString>,
) -> Result<std::process::Output, String> {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.args(&args)
        .env("HOME", install::tunnel_dir(dirs))
        .env("TUNNEL_ORIGIN_CERT", origincert(dirs))
        .stdin(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(ONESHOT_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(format!("spawn cloudflared: {e}")),
        Err(_) => Err("cloudflared timed out".into()),
    }
}

/// The trailing line of a process's stderr/stdout, for error messages.
fn last_error_line(out: &std::process::Output) -> String {
    let text = String::from_utf8_lossy(&out.stderr);
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("cloudflared failed")
        .to_owned()
}

/// `CloudflaredLogin` - run `cloudflared tunnel login` as a streamed job,
/// surfacing the one-time auth URL in the job log for the GUI to open. Succeeds
/// once the account cert lands on disk.
pub async fn login_streamed(state: Arc<DaemonState>) -> Response {
    let Some(resolved) = super::resolved_cloudflared(&state).await else {
        return not_installed();
    };
    if let Err(e) = ensure_secret_dirs(&state.dirs) {
        return internal(e);
    }
    let (job_id, mut cancel) = state.jobs.create().await;
    let id = job_id.clone();
    tokio::spawn(async move {
        let _guard = state.tunnel_mutate.lock().await;
        state
            .jobs
            .set_phase(&id, "Waiting for Cloudflare login")
            .await;
        let binary = resolved.binary;
        let args = yerd_tunnel::args::login_args(&origincert(&state.dirs));
        let mut cmd = tokio::process::Command::new(&binary);
        cmd.args(&args)
            .env("HOME", install::tunnel_dir(&state.dirs))
            .env("TUNNEL_ORIGIN_CERT", origincert(&state.dirs))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                state
                    .jobs
                    .finish(&id, yerd_ipc::JobState::Failed, Some(format!("spawn: {e}")))
                    .await;
                return;
            }
        };

        let mut tasks = Vec::new();
        if let Some(out) = child.stdout.take() {
            tasks.push(tokio::spawn(drain_lines(state.clone(), id.clone(), out)));
        }
        if let Some(err) = child.stderr.take() {
            tasks.push(tokio::spawn(drain_lines(state.clone(), id.clone(), err)));
        }

        let end = tokio::select! {
            _ = child.wait() => LoginEnd::Exited,
            _ = cancel.changed() => { let _ = child.kill().await; LoginEnd::Cancelled }
            () = tokio::time::sleep(LOGIN_TIMEOUT) => { let _ = child.kill().await; LoginEnd::TimedOut }
        };
        for t in tasks {
            let _ = t.await;
        }

        if matches!(end, LoginEnd::Cancelled) {
            state
                .jobs
                .finish(&id, yerd_ipc::JobState::Cancelled, None)
                .await;
        } else if matches!(end, LoginEnd::TimedOut) {
            state
                .jobs
                .finish(
                    &id,
                    yerd_ipc::JobState::Failed,
                    Some("login timed out - the browser authorization wasn't completed".into()),
                )
                .await;
        } else if is_logged_in(&state.dirs) {
            if let Err(e) = crate::secure_fs::restrict_to_owner(&origincert(&state.dirs)) {
                tracing::warn!(error = %e, "could not tighten permissions on cloudflared cert");
            }
            state
                .jobs
                .finish(&id, yerd_ipc::JobState::Succeeded, None)
                .await;
        } else {
            state
                .jobs
                .finish(
                    &id,
                    yerd_ipc::JobState::Failed,
                    Some("login did not complete (no certificate written)".into()),
                )
                .await;
        }
    });
    Response::JobStarted { job_id }
}

/// Push each line of a child stream to the job log, surfacing any login URL.
async fn drain_lines<R>(state: Arc<DaemonState>, id: yerd_ipc::JobId, reader: R)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(url) = find_auth_url(&line) {
            state
                .jobs
                .push_log(&id, format!("Open this URL to authorize Yerd: {url}"))
                .await;
        }
        state.jobs.push_log(&id, line).await;
    }
}

/// `CreateNamedTunnel` - create a tunnel on the logged-in account and record its
/// UUID. v1 supports a single tunnel: creating a second (differently-named) one
/// is rejected. Re-running with the existing name is left to Cloudflare.
pub async fn create(name: &str, state: &DaemonState) -> Response {
    let Some(resolved) = super::resolved_cloudflared(state).await else {
        return not_installed();
    };
    if !is_logged_in(&state.dirs) {
        return need_login();
    }
    if !is_valid_tunnel_name(name) {
        return Response::Error {
            code: ErrorCode::InvalidPath,
            message: "invalid tunnel name - use letters, digits, '-', '_' or '.'".into(),
        };
    }
    if let Err(e) = ensure_secret_dirs(&state.dirs) {
        return internal(e);
    }

    let creds = creds_file(&state.dirs, name);
    let _guard = state.tunnel_mutate.lock().await;
    if state
        .config
        .lock()
        .await
        .tunnel
        .named
        .keys()
        .any(|n| n != name)
    {
        return Response::Error {
            code: ErrorCode::AlreadyExists,
            message: "a named tunnel already exists - Yerd supports one tunnel; \
                      remove the existing one first"
                .into(),
        };
    }
    let args = yerd_tunnel::args::create_args(name, &origincert(&state.dirs), &creds);
    let out = match run_oneshot(&state.dirs, &resolved.binary, args).await {
        Ok(o) => o,
        Err(e) => return internal(e),
    };
    if !out.status.success() {
        return internal(format!("create tunnel failed: {}", last_error_line(&out)));
    }
    if let Err(e) = crate::secure_fs::restrict_to_owner(&creds) {
        tracing::warn!(error = %e, "could not tighten permissions on tunnel credentials");
    }

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let Some(uuid) = find_tunnel_id(&combined) else {
        return internal("could not parse tunnel id from cloudflared output".into());
    };

    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    new.tunnel.named.insert(name.to_owned(), uuid.clone());
    if let Err(e) = new.validate() {
        return internal(format!("config validation failed: {e}"));
    }
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *cfg_guard = new;
    tracing::info!(name, uuid = %uuid, "created named tunnel");
    Response::Ok
}

/// `ListNamedTunnels` - the named tunnels recorded locally, the per-site hostname
/// mappings enabled in the consolidated tunnel, and the authorized Cloudflare
/// zone. The zone is resolved via a bounded, cached Cloudflare API call (see
/// [`super::credentials::resolve_zone`]); everything else is read from config.
pub async fn list(state: &DaemonState) -> Response {
    let (tunnels, raw_sites, raw_passwords) = {
        let cfg = state.config.lock().await;
        let tunnels: Vec<NamedTunnelMeta> = cfg
            .tunnel
            .named
            .iter()
            .map(|(name, uuid)| NamedTunnelMeta {
                name: name.clone(),
                uuid: uuid.clone(),
            })
            .collect();
        (
            tunnels,
            cfg.tunnel.sites.clone(),
            cfg.tunnel.passwords.clone(),
        )
    };
    let sites = {
        let router = state.router.read().await;
        list_site_hostnames(&raw_sites, &raw_passwords, |key| {
            super::canonical_in(&router, key)
        })
    };
    let zone = if is_logged_in(&state.dirs) {
        super::credentials::resolve_zone(&state.dirs).await
    } else {
        None
    };
    Response::NamedTunnels {
        tunnels,
        sites,
        zone,
    }
}

/// `RouteTunnelDns` - create the proxied CNAME routing `hostname` to `tunnel`.
pub async fn route_dns(tunnel: &str, hostname: &str, state: &DaemonState) -> Response {
    let Some(resolved) = super::resolved_cloudflared(state).await else {
        return not_installed();
    };
    if !is_logged_in(&state.dirs) {
        return need_login();
    }
    if !is_valid_tunnel_name(tunnel) || !is_safe_cli_value(hostname) {
        return Response::Error {
            code: ErrorCode::InvalidPath,
            message: "invalid tunnel name or hostname".into(),
        };
    }
    let args = yerd_tunnel::args::route_dns_args(tunnel, hostname, &origincert(&state.dirs));
    match run_oneshot(&state.dirs, &resolved.binary, args).await {
        Ok(out) if out.status.success() => Response::Ok,
        Ok(out) => internal(format!("route dns failed: {}", last_error_line(&out))),
        Err(e) => internal(e),
    }
}

/// Resolve the `site` argument of a per-site tunnel setting (a site name or its
/// `.test` host) to the canonical site name the config is keyed by. When
/// `setting`, the site must be shareable ([`super::resolve_site`]) or the reply
/// is `NotFound`; when clearing, an unresolvable key is `Ok(None)` so a stale
/// entry can still be removed under its raw key.
async fn site_key(
    state: &DaemonState,
    site: &str,
    setting: bool,
) -> Result<Option<String>, Response> {
    if !setting {
        return Ok(super::canonical_site_name(state, site).await);
    }
    match super::resolve_site(state, site).await {
        Some((name, _host)) => Ok(Some(name)),
        None => Err(Response::Error {
            code: ErrorCode::NotFound,
            message: format!("no site named {site:?}"),
        }),
    }
}

/// `SetSiteTunnel` - persist (or clear, with `None`) a site's public hostname.
///
/// The entry is stored under the canonical site name, replacing any entry under
/// the raw argument or another key that resolves to the same site (a legacy
/// `.test` host key).
///
/// Setting a hostname is rejected for a site that does not exist, so the
/// persisted map can never accumulate entries that `list` would show but the
/// tunnel listener silently drops. Clearing (`None`) is always allowed so a
/// stale entry for a since-removed site can still be cleaned up. The tunnel
/// route map is rebuilt once the config lock is released, so the change takes
/// effect on the next request without touching `cloudflared`.
///
/// Setting a hostname refreshes that site's share warnings and ensures the named
/// tunnel is running in the background (never restarting a live one); a failure
/// there, such as no login yet, is logged and does not fail the save, and a stop
/// that completes after this request began wins over it. Clearing
/// the last hostname stops the named tunnel, under `tunnel_mutate` so a
/// concurrent [`ensure_running`] cannot slip in between the check and the stop.
pub async fn set_site_hostname(
    site: &str,
    hostname: Option<&str>,
    state: &Arc<DaemonState>,
) -> Response {
    let epoch = state.named_stop_epoch.load(Ordering::SeqCst);
    let canonical = match site_key(state, site, hostname.is_some()).await {
        Ok(canonical) => canonical,
        Err(resp) => return resp,
    };
    let aliases = match &canonical {
        Some(name) => super::tunnel_keys_for(state, name).await,
        None => Vec::new(),
    };
    {
        let mut cfg_guard = state.config.lock().await;
        let mut new = cfg_guard.clone();
        new.tunnel.sites.remove(site);
        for key in &aliases {
            new.tunnel.sites.remove(key);
        }
        if let (Some(h), Some(name)) = (hostname, &canonical) {
            new.tunnel.sites.insert(name.clone(), h.to_owned());
        }
        if let Err(e) = new.validate() {
            return Response::Error {
                code: ErrorCode::InvalidPath,
                message: format!("invalid hostname: {e}"),
            };
        }
        if let Err(e) = new.save(&state.config_path) {
            return internal(format!("config save failed: {e}"));
        }
        *cfg_guard = new;
    }
    super::rebuild_routes(state).await;
    if let (Some(_), Some(name)) = (hostname, &canonical) {
        super::refresh_share_warnings(state, name).await;
        tokio::spawn(ensure_in_background(Arc::clone(state), epoch));
        return Response::Ok;
    }
    let _guard = state.tunnel_mutate.lock().await;
    let none_left = state.config.lock().await.tunnel.sites.is_empty();
    if none_left {
        state.named_stop_epoch.fetch_add(1, Ordering::SeqCst);
        if let Err(e) = state.tunnel_manager.lock().await.stop(NAMED_KEY).await {
            return internal(format!("could not stop the named tunnel: {e}"));
        }
    }
    Response::Ok
}

/// `SetTunnelPassword` - persist (or clear, with `None`) a site's share
/// password, keyed by canonical site name so a `.test` host argument protects
/// the site it names. Mirrors [`set_site_hostname`]: setting a password is rejected for a
/// site that does not exist, clearing is always allowed so a stale entry can be
/// removed. The new value is validated (1..=128 bytes, no control characters)
/// and saved under the config lock; the tunnel route map is rebuilt once that
/// lock is released, so the password takes effect on the next request.
pub async fn set_password(site: &str, password: Option<&str>, state: &DaemonState) -> Response {
    let canonical = match site_key(state, site, password.is_some()).await {
        Ok(canonical) => canonical,
        Err(resp) => return resp,
    };
    let aliases = match &canonical {
        Some(name) => super::tunnel_keys_for(state, name).await,
        None => Vec::new(),
    };
    {
        let mut cfg_guard = state.config.lock().await;
        let mut new = cfg_guard.clone();
        new.tunnel.passwords.remove(site);
        for key in &aliases {
            new.tunnel.passwords.remove(key);
        }
        if let (Some(p), Some(name)) = (password, &canonical) {
            new.tunnel.passwords.insert(name.clone(), p.to_owned());
        }
        if let Err(e) = new.validate() {
            return Response::Error {
                code: ErrorCode::InvalidPath,
                message: format!("invalid share password: {e}"),
            };
        }
        if let Err(e) = new.save(&state.config_path) {
            return internal(format!("config save failed: {e}"));
        }
        *cfg_guard = new;
    }
    super::rebuild_routes(state).await;
    Response::Ok
}

/// Why [`ensure_running`] did not leave a named tunnel running.
#[derive(Debug)]
enum EnsureFailure {
    /// The tunnel manager has shut down; expected during daemon exit.
    ShuttingDown,
    /// The named tunnel was stopped after this start was queued.
    Superseded,
    /// A precondition or setup step failed, with the reply for an IPC caller.
    Reply(Response),
}

/// The facts deciding whether the named tunnel can run.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NamedFacts {
    /// A Cloudflare login (origin cert) is present.
    pub logged_in: bool,
    /// A `cloudflared` binary resolved.
    pub installed: bool,
    /// A named tunnel is recorded in config.
    pub has_tunnel: bool,
    /// How many sites have a public hostname.
    pub site_count: usize,
    /// The tunnel origin listener is bound.
    pub has_origin: bool,
}

/// The reason the named tunnel cannot run yet, as an IPC reply, or `None` when
/// every precondition holds: `cloudflared` resolved, a Cloudflare login, a
/// recorded tunnel, at least one exposed site, and a bound tunnel listener.
fn named_blocker(facts: NamedFacts) -> Option<Response> {
    if !facts.installed {
        return Some(not_installed());
    }
    if !facts.logged_in {
        return Some(need_login());
    }
    if !facts.has_tunnel {
        return Some(Response::Error {
            code: ErrorCode::NotFound,
            message: "no named tunnel created yet - create one first".into(),
        });
    }
    if facts.site_count == 0 {
        return Some(Response::Error {
            code: ErrorCode::NotFound,
            message: "no sites enabled - set a hostname for a site first".into(),
        });
    }
    if !facts.has_origin {
        return Some(super::origin_not_running());
    }
    None
}

/// Whether the named tunnel should be running given these facts. Pure; see
/// [`named_blocker`] for the individual preconditions.
pub(super) fn should_run_named(facts: NamedFacts) -> bool {
    named_blocker(facts).is_none()
}

/// The facts readable without resolving `cloudflared` (which may probe a
/// system binary), assuming it is installed. Takes and releases the config
/// lock. Returns them with the recorded tunnel's `(name, uuid)`.
async fn cheap_facts(state: &DaemonState) -> (NamedFacts, Option<(String, String)>) {
    let (tunnel, site_count) = named_config(state).await;
    let facts = NamedFacts {
        logged_in: is_logged_in(&state.dirs),
        installed: true,
        has_tunnel: tunnel.is_some(),
        site_count,
        has_origin: state.tunnel_origin_port.is_some(),
    };
    (facts, tunnel)
}

/// The recorded tunnel's `(name, uuid)` and the number of exposed sites.
async fn named_config(state: &DaemonState) -> (Option<(String, String)>, usize) {
    let cfg = state.config.lock().await;
    let tunnel = cfg
        .tunnel
        .named
        .iter()
        .next()
        .map(|(n, u)| (n.clone(), u.clone()));
    (tunnel, cfg.tunnel.sites.len())
}

/// Ensure the consolidated named tunnel is running, starting it when absent.
///
/// Idempotent: a live or retrying instance is left alone (the manager's `begin`
/// refuses to replace it), so exposing or unexposing a site never restarts the
/// connector. The preconditions are re-checked under `tunnel_mutate`, because a
/// concurrent delete or hostname clear may have run while this call waited for
/// it; `named.yml` (one catch-all rule pointing at the tunnel listener) is
/// rendered and the instance launched under that same guard.
///
/// Returns the generation of the instance now running, or `None` if it was
/// stopped again before it could be read.
///
/// # Errors
///
/// An IPC error reply when a precondition fails, setup fails, or the daemon is
/// shutting down.
pub async fn ensure_running(state: &Arc<DaemonState>) -> Result<Option<u64>, Response> {
    ensure(state, None).await.map_err(|failure| match failure {
        EnsureFailure::Superseded => internal("the named tunnel was stopped".into()),
        EnsureFailure::ShuttingDown => internal(format!(
            "could not start the named tunnel: {}",
            yerd_tunnel::TunnelError::ShuttingDown
        )),
        EnsureFailure::Reply(resp) => resp,
    })
}

/// [`ensure_running`] for background callers: a failure is logged, never
/// returned. Shutdown and unmet preconditions are expected and logged at debug.
/// `epoch` is the `named_stop_epoch` seen when the start was queued; a stop
/// since then cancels it.
pub(super) async fn ensure_in_background(state: Arc<DaemonState>, epoch: u64) {
    match ensure(&state, Some(epoch)).await {
        Ok(_) => {}
        Err(EnsureFailure::Superseded) => {
            tracing::debug!("named tunnel not started: it was stopped after the start was queued");
        }
        Err(EnsureFailure::ShuttingDown) => {
            tracing::debug!("named tunnel not started: daemon is shutting down");
        }
        Err(EnsureFailure::Reply(resp)) => {
            tracing::debug!(reply = ?resp, "named tunnel not started");
        }
    }
}

async fn ensure(
    state: &Arc<DaemonState>,
    epoch: Option<u64>,
) -> Result<Option<u64>, EnsureFailure> {
    if let Some(resp) = named_blocker(cheap_facts(state).await.0) {
        return Err(EnsureFailure::Reply(resp));
    }
    let Some(resolved) = super::resolved_cloudflared(state).await else {
        return Err(EnsureFailure::Reply(not_installed()));
    };

    let guard = state.tunnel_mutate.lock().await;
    if epoch.is_some_and(|e| e != state.named_stop_epoch.load(Ordering::SeqCst)) {
        return Err(EnsureFailure::Superseded);
    }
    let (facts, tunnel) = cheap_facts(state).await;
    if let Some(resp) = named_blocker(facts) {
        return Err(EnsureFailure::Reply(resp));
    }
    let (Some((tunnel_name, uuid)), Some(port)) = (tunnel, state.tunnel_origin_port) else {
        return Err(EnsureFailure::Reply(super::origin_not_running()));
    };

    let creds = creds_file(&state.dirs, &tunnel_name);
    let config_yml =
        yerd_tunnel::config::render_ingress_config(&uuid, &creds, &yerd_tunnel::origin_url(port));
    let config_path = install::tunnel_dir(&state.dirs).join("named.yml");
    ensure_secret_dirs(&state.dirs).map_err(|e| EnsureFailure::Reply(internal(e)))?;
    yerd_php::io::atomic_write::write(&config_path, config_yml.as_bytes())
        .map_err(|e| EnsureFailure::Reply(internal(format!("write tunnel config: {e}"))))?;

    let args = yerd_tunnel::args::named_run_args(&config_path, &origincert(&state.dirs));
    let mut env = super::pinned_home_env(state);
    env.push((
        "TUNNEL_ORIGIN_CERT".into(),
        origincert(&state.dirs).into_os_string(),
    ));
    let spec = super::launch_spec(state, NAMED_KEY, resolved.binary, args, env)
        .map_err(EnsureFailure::Reply)?;
    let launched = super::supervise::launch(state, NAMED_KEY, spec, TunnelKind::Named)
        .await
        .map_err(|e| match e {
            yerd_tunnel::TunnelError::ShuttingDown => EnsureFailure::ShuttingDown,
            other => EnsureFailure::Reply(internal(format!(
                "could not start the named tunnel: {other}"
            ))),
        })?;
    drop(guard);

    Ok(match launched {
        Some(l) => Some(l.generation),
        None => super::supervise::current_generation(state, NAMED_KEY).await,
    })
}

/// Start the named tunnel at daemon start when [`should_run_named`] holds,
/// refreshing every mapped site's share warnings first. `epoch` is the
/// `named_stop_epoch` seen when the resume was queued, so a stop that completes
/// in the meantime keeps the tunnel down. Errors are logged and swallowed;
/// shutdown racing the start is expected and logged at debug.
pub(super) async fn resume(state: Arc<DaemonState>, epoch: u64) {
    let wanted = should_run_named(cheap_facts(&state).await.0)
        && super::resolved_cloudflared(&state).await.is_some();
    if !wanted {
        tracing::debug!("named tunnel not resumed: nothing to share or not set up");
        return;
    }
    super::refresh_named_share_warnings(&state).await;
    match ensure(&state, Some(epoch)).await {
        Ok(_) => tracing::info!("resumed named tunnel"),
        Err(EnsureFailure::Superseded) => {
            tracing::debug!("named tunnel not resumed: it was stopped meanwhile");
        }
        Err(EnsureFailure::ShuttingDown) => {
            tracing::debug!("named tunnel not resumed: daemon is shutting down");
        }
        Err(EnsureFailure::Reply(resp)) => {
            tracing::warn!(reply = ?resp, "could not resume named tunnel");
        }
    }
}

/// `StartNamedTunnel` - ensure the single consolidated named tunnel serving
/// every enabled site is running (see [`ensure_running`]); it never restarts a
/// live connector. Every mapped site's share warnings are refreshed first. The
/// call then waits for the instance to settle; the named tunnel keeps retrying
/// in the background, so one that does not come online in time is reported as
/// the live (failed or connecting) tunnel with its `last_error`, not an error.
pub async fn start(state: &Arc<DaemonState>) -> Response {
    super::refresh_named_share_warnings(state).await;
    let generation = match ensure_running(state).await {
        Ok(generation) => generation,
        Err(resp) => return resp,
    };
    if let Some(generation) = generation {
        super::supervise::wait_settled(
            state,
            NAMED_KEY,
            generation,
            super::supervise::SETTLE_TIMEOUT,
        )
        .await;
    }
    super::tunnels_response(state).await
}

/// `StopNamedTunnel` - tear down the consolidated named tunnel.
///
/// Runtime-only: the hostnames stay configured, so the tunnel starts again on
/// the next hostname change or daemon start. The stop runs under
/// `tunnel_mutate` and bumps `named_stop_epoch`, so a background start queued
/// before this call cannot relaunch the tunnel after it returns.
pub async fn stop(state: &DaemonState) -> Response {
    {
        let _guard = state.tunnel_mutate.lock().await;
        state.named_stop_epoch.fetch_add(1, Ordering::SeqCst);
        let mut mgr = state.tunnel_manager.lock().await;
        if let Err(e) = mgr.stop(NAMED_KEY).await {
            return internal(format!("could not stop named tunnel: {e}"));
        }
    }
    super::tunnels_response(state).await
}

/// `DeleteNamedTunnel` - delete `name` from the Cloudflare account and forget it
/// locally. Refuses any `name` that isn't the one recorded tunnel (v1 is
/// single-tunnel), so a stray request can't destroy an unrelated account tunnel.
/// Stops the running process, runs `cloudflared tunnel cleanup` (so the
/// just-stopped edge connections don't block deletion) then `tunnel delete`,
/// removes the credentials file, and clears the persisted tunnel + site mappings
/// (the per-site DNS records point at the now-deleted tunnel, so the clean slate
/// avoids re-routing a stale UUID; the records themselves remain on the account
/// and need removing in the Cloudflare dashboard). Best-effort cleanup; a failed
/// `delete` is surfaced.
pub async fn delete(name: &str, state: &DaemonState) -> Response {
    let Some(resolved) = super::resolved_cloudflared(state).await else {
        return not_installed();
    };
    if !is_logged_in(&state.dirs) {
        return need_login();
    }
    if !is_valid_tunnel_name(name) {
        return Response::Error {
            code: ErrorCode::InvalidPath,
            message: "invalid tunnel name".into(),
        };
    }

    let _guard = state.tunnel_mutate.lock().await;

    let is_configured = state
        .config
        .lock()
        .await
        .tunnel
        .named
        .keys()
        .any(|n| n == name);
    if !is_configured {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: "no such named tunnel".into(),
        };
    }

    state.named_stop_epoch.fetch_add(1, Ordering::SeqCst);
    if let Err(e) = state.tunnel_manager.lock().await.stop(NAMED_KEY).await {
        return internal(format!(
            "could not stop the named tunnel before deleting it: {e}"
        ));
    }

    let cert = origincert(&state.dirs);
    let _ = run_oneshot(
        &state.dirs,
        &resolved.binary,
        yerd_tunnel::args::cleanup_args(name, &cert),
    )
    .await;
    match run_oneshot(
        &state.dirs,
        &resolved.binary,
        yerd_tunnel::args::delete_args(name, &cert),
    )
    .await
    {
        Ok(out) if out.status.success() => {}
        Ok(out) => return internal(format!("delete tunnel failed: {}", last_error_line(&out))),
        Err(e) => return internal(e),
    }

    let _ = std::fs::remove_file(creds_file(&state.dirs, name));

    {
        let mut cfg_guard = state.config.lock().await;
        let mut new = cfg_guard.clone();
        new.tunnel.named.remove(name);
        new.tunnel.sites.clear();
        if let Err(e) = new.validate() {
            return internal(format!("config validation failed: {e}"));
        }
        if let Err(e) = new.save(&state.config_path) {
            return internal(format!("config save failed: {e}"));
        }
        *cfg_guard = new;
    }
    super::rebuild_routes(state).await;
    tracing::info!(name, "deleted named tunnel");
    Response::Ok
}

/// The `ListNamedTunnels` site rows: each `[tunnel.sites]` key reported under
/// its canonical site name when `resolve` knows it (so a legacy `.test` host
/// key shows as its site), or verbatim when it names no site (so a stale entry
/// stays visible and clearable). An exact-name key wins over a host alias of
/// the same site. A row is protected when any password key resolves to it.
fn list_site_hostnames(
    sites: &std::collections::BTreeMap<String, String>,
    passwords: &std::collections::BTreeMap<String, String>,
    resolve: impl Fn(&str) -> Option<String>,
) -> Vec<SiteHostname> {
    let canonical = |key: &str| resolve(key).unwrap_or_else(|| key.to_owned());
    let protected: std::collections::BTreeSet<String> =
        passwords.keys().map(|k| canonical(k)).collect();
    let mut rows: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (key, hostname) in sites {
        let name = canonical(key);
        if name == *key || !rows.contains_key(&name) {
            rows.insert(name, hostname.clone());
        }
    }
    rows.into_iter()
        .map(|(site, hostname)| SiteHostname {
            protected: protected.contains(&site),
            site,
            hostname,
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::test_support::state_in;
    use yerd_core::{PhpVersion, Site};

    async fn state_with_site(tmp: &Path, name: &str) -> Arc<DaemonState> {
        let state = state_in(tmp);
        let site = Site::linked(name, "/srv/www/blog", PhpVersion::new(8, 3)).unwrap();
        state.router.write().await.insert(site).unwrap();
        Arc::new(state)
    }

    #[test]
    fn list_rows_report_canonical_names_and_keep_stale_keys() {
        let map = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let resolve = |key: &str| match key {
            "app" | "app.test" => Some("app".to_owned()),
            "blog" => Some("blog".to_owned()),
            _ => None,
        };
        let sites = map(&[
            ("app.test", "old.example.com"),
            ("app", "app.example.com"),
            ("blog", "blog.example.com"),
            ("gone", "gone.example.com"),
        ]);
        let passwords = map(&[("app.test", "pw")]);
        let rows = list_site_hostnames(&sites, &passwords, resolve);
        let got: Vec<(&str, &str, bool)> = rows
            .iter()
            .map(|r| (r.site.as_str(), r.hostname.as_str(), r.protected))
            .collect();
        assert_eq!(
            got,
            vec![
                ("app", "app.example.com", true),
                ("blog", "blog.example.com", false),
                ("gone", "gone.example.com", false),
            ]
        );
    }

    #[test]
    fn list_rows_map_a_legacy_host_key_to_its_site() {
        let sites: std::collections::BTreeMap<String, String> =
            [("app.test".to_owned(), "app.example.com".to_owned())].into();
        let rows = list_site_hostnames(&sites, &std::collections::BTreeMap::new(), |k| {
            (k == "app.test").then(|| "app".to_owned())
        });
        assert_eq!(rows.len(), 1);
        assert_eq!(rows.first().map(|r| r.site.as_str()), Some("app"));
    }

    const ALL: NamedFacts = NamedFacts {
        logged_in: true,
        installed: true,
        has_tunnel: true,
        site_count: 1,
        has_origin: true,
    };

    #[test]
    fn should_run_named_table() {
        let cases: &[(NamedFacts, bool)] = &[
            (ALL, true),
            (
                NamedFacts {
                    site_count: 3,
                    ..ALL
                },
                true,
            ),
            (
                NamedFacts {
                    logged_in: false,
                    ..ALL
                },
                false,
            ),
            (
                NamedFacts {
                    installed: false,
                    ..ALL
                },
                false,
            ),
            (
                NamedFacts {
                    has_tunnel: false,
                    ..ALL
                },
                false,
            ),
            (
                NamedFacts {
                    site_count: 0,
                    ..ALL
                },
                false,
            ),
            (
                NamedFacts {
                    has_origin: false,
                    ..ALL
                },
                false,
            ),
        ];
        for (facts, expected) in cases {
            assert_eq!(should_run_named(*facts), *expected, "{facts:?}");
        }
    }

    #[test]
    fn named_blocker_names_the_first_missing_precondition() {
        let message = |facts: NamedFacts| match named_blocker(facts) {
            Some(Response::Error { message, .. }) => message,
            other => panic!("expected an error, got {other:?}"),
        };
        let cases: &[(NamedFacts, &str)] = &[
            (
                NamedFacts {
                    installed: false,
                    logged_in: false,
                    ..ALL
                },
                "not installed",
            ),
            (
                NamedFacts {
                    logged_in: false,
                    ..ALL
                },
                "not logged in",
            ),
            (
                NamedFacts {
                    has_tunnel: false,
                    ..ALL
                },
                "no named tunnel",
            ),
            (
                NamedFacts {
                    site_count: 0,
                    ..ALL
                },
                "no sites enabled",
            ),
            (
                NamedFacts {
                    has_origin: false,
                    ..ALL
                },
                "origin listener",
            ),
        ];
        for (facts, expected) in cases {
            assert!(message(*facts).contains(expected), "{facts:?}");
        }
    }

    #[tokio::test]
    async fn list_reports_protected_sites() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_with_site(tmp.path(), "blog").await;
        assert_eq!(
            set_site_hostname("blog", Some("blog.example.com"), &state).await,
            Response::Ok
        );
        assert_eq!(
            set_password("blog", Some("secret"), &state).await,
            Response::Ok
        );

        let Response::NamedTunnels { sites, .. } = list(&state).await else {
            panic!("expected NamedTunnels");
        };

        assert_eq!(
            sites,
            vec![SiteHostname {
                site: "blog".to_owned(),
                hostname: "blog.example.com".to_owned(),
                protected: true,
            }]
        );
    }

    fn saved_passwords(state: &DaemonState) -> std::collections::BTreeMap<String, String> {
        yerd_config::Config::load(&state.config_path)
            .unwrap()
            .tunnel
            .passwords
    }

    #[tokio::test]
    async fn set_password_persists_to_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_with_site(tmp.path(), "blog").await;

        let resp = set_password("blog", Some("secret"), &state).await;

        assert_eq!(resp, Response::Ok);
        assert_eq!(
            saved_passwords(&state).get("blog").map(String::as_str),
            Some("secret")
        );
        assert_eq!(
            state
                .config
                .lock()
                .await
                .tunnel
                .passwords
                .get("blog")
                .map(String::as_str),
            Some("secret")
        );
    }

    #[tokio::test]
    async fn hostname_and_password_changes_rebuild_the_route_map() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_with_site(tmp.path(), "blog").await;

        assert_eq!(
            set_site_hostname("blog", Some("blog.example.com"), &state).await,
            Response::Ok
        );
        assert_eq!(
            set_password("blog", Some("secret"), &state).await,
            Response::Ok
        );
        assert_eq!(
            state.tunnel_routes.read().await.get("blog.example.com"),
            Some(&yerd_proxy::TunnelRoute {
                site: "blog".to_owned(),
                password: Some("secret".to_owned()),
            })
        );

        assert_eq!(set_site_hostname("blog", None, &state).await, Response::Ok);
        assert!(state.tunnel_routes.read().await.is_empty());
    }

    fn saved_sites(state: &DaemonState) -> std::collections::BTreeMap<String, String> {
        yerd_config::Config::load(&state.config_path)
            .unwrap()
            .tunnel
            .sites
    }

    fn pairs(list: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        list.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[tokio::test]
    async fn a_password_set_by_host_protects_the_site() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_with_site(tmp.path(), "blog").await;
        assert_eq!(
            set_site_hostname("blog", Some("blog.example.com"), &state).await,
            Response::Ok
        );

        let resp = set_password("blog.test", Some("secret"), &state).await;

        assert_eq!(resp, Response::Ok);
        assert_eq!(saved_passwords(&state), pairs(&[("blog", "secret")]));
        assert_eq!(
            state.tunnel_routes.read().await.get("blog.example.com"),
            Some(&yerd_proxy::TunnelRoute {
                site: "blog".to_owned(),
                password: Some("secret".to_owned()),
            })
        );
    }

    #[tokio::test]
    async fn clearing_a_password_by_host_removes_the_canonical_and_legacy_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_with_site(tmp.path(), "blog").await;
        state.config.lock().await.tunnel.passwords =
            pairs(&[("blog", "new"), ("blog.test", "old")]);

        let resp = set_password("blog.test", None, &state).await;

        assert_eq!(resp, Response::Ok);
        assert!(saved_passwords(&state).is_empty());
    }

    #[tokio::test]
    async fn a_hostname_is_stored_under_the_canonical_name_replacing_legacy_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_with_site(tmp.path(), "blog").await;
        state.config.lock().await.tunnel.sites = pairs(&[("blog.test", "blog.example.com")]);

        let resp = set_site_hostname("blog", Some("blog.example.com"), &state).await;
        assert_eq!(resp, Response::Ok);
        assert_eq!(saved_sites(&state), pairs(&[("blog", "blog.example.com")]));

        let resp = set_site_hostname("blog.test", Some("www.example.com"), &state).await;
        assert_eq!(resp, Response::Ok);
        assert_eq!(saved_sites(&state), pairs(&[("blog", "www.example.com")]));
        assert_eq!(
            state.tunnel_routes.read().await.get("www.example.com"),
            Some(&yerd_proxy::TunnelRoute {
                site: "blog".to_owned(),
                password: None,
            })
        );

        assert_eq!(
            set_site_hostname("blog.test", None, &state).await,
            Response::Ok
        );
        assert!(saved_sites(&state).is_empty());
    }

    #[tokio::test]
    async fn stop_waits_for_tunnel_mutate() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_with_site(tmp.path(), "blog").await;
        install::install_binary_for_test(&state.dirs, "2026.6.1", b"#!/bin/sh\nexit 0\n");
        super::super::refresh_cloudflared_cache_after_install(&state).await;
        let guard = state.tunnel_mutate.lock().await;
        let stopping = {
            let state = Arc::clone(&state);
            tokio::spawn(async move { stop(&state).await })
        };

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!stopping.is_finished());
        drop(guard);

        let resp = tokio::time::timeout(std::time::Duration::from_secs(5), stopping)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(resp, Response::Tunnels { .. }), "{resp:?}");
    }

    #[tokio::test]
    async fn a_background_start_queued_before_a_stop_is_superseded() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = state_in(tmp.path());
        state.tunnel_origin_port = Some(1);
        let state = Arc::new(state);
        install::install_binary_for_test(&state.dirs, "2026.6.1", b"#!/bin/sh\nexit 0\n");
        super::super::refresh_cloudflared_cache_after_install(&state).await;
        std::fs::create_dir_all(origincert(&state.dirs).parent().unwrap()).unwrap();
        std::fs::write(origincert(&state.dirs), "cert").unwrap();
        {
            let mut cfg = state.config.lock().await;
            cfg.tunnel.named.insert("t".into(), "uuid".into());
            cfg.tunnel
                .sites
                .insert("blog".into(), "blog.example.com".into());
        }
        let epoch = state.named_stop_epoch.load(Ordering::SeqCst);

        assert!(matches!(stop(&state).await, Response::Tunnels { .. }));
        let result = ensure(&state, Some(epoch)).await;

        assert!(
            matches!(result, Err(EnsureFailure::Superseded)),
            "{result:?}"
        );
        assert!(state.tunnel_manager.lock().await.snapshots().is_empty());
    }

    #[tokio::test]
    async fn a_stop_during_a_hostname_request_cancels_its_background_start() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = state_in(tmp.path());
        state.tunnel_origin_port = Some(1);
        let site = Site::linked("blog", "/srv/www/blog", PhpVersion::new(8, 3)).unwrap();
        state.router.write().await.insert(site).unwrap();
        let state = Arc::new(state);
        install::install_binary_for_test(&state.dirs, "2026.6.1", b"#!/bin/sh\nsleep 30\n");
        super::super::refresh_cloudflared_cache_after_install(&state).await;
        std::fs::create_dir_all(origincert(&state.dirs).parent().unwrap()).unwrap();
        std::fs::write(origincert(&state.dirs), "cert").unwrap();
        state
            .config
            .lock()
            .await
            .tunnel
            .named
            .insert("t".into(), "uuid".into());

        let config = state.config.lock().await;
        let request = {
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                set_site_hostname("blog", Some("blog.example.com"), &state).await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let stopping = {
            let state = Arc::clone(&state);
            tokio::spawn(async move { stop(&state).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drop(config);

        assert!(matches!(stopping.await.unwrap(), Response::Tunnels { .. }));
        assert_eq!(request.await.unwrap(), Response::Ok);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        assert!(state.tunnel_manager.lock().await.snapshots().is_empty());
        state.tunnel_manager.lock().await.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn clear_password_removes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_with_site(tmp.path(), "blog").await;
        assert_eq!(
            set_password("blog", Some("secret"), &state).await,
            Response::Ok
        );

        let resp = set_password("blog", None, &state).await;

        assert_eq!(resp, Response::Ok);
        assert!(saved_passwords(&state).is_empty());
        assert!(state.config.lock().await.tunnel.passwords.is_empty());
    }

    #[tokio::test]
    async fn clear_password_is_allowed_for_an_unknown_site() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        assert_eq!(set_password("gone", None, &state).await, Response::Ok);
    }

    #[tokio::test]
    async fn too_long_password_is_rejected_without_saving() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_with_site(tmp.path(), "blog").await;
        let long = "x".repeat(129);

        let resp = set_password("blog", Some(&long), &state).await;

        assert!(
            matches!(
                resp,
                Response::Error {
                    code: ErrorCode::InvalidPath,
                    ..
                }
            ),
            "{resp:?}"
        );
        assert!(!state.config_path.exists());
        assert!(state.config.lock().await.tunnel.passwords.is_empty());
    }

    #[tokio::test]
    async fn password_for_an_unknown_site_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        let resp = set_password("nope", Some("secret"), &state).await;

        assert!(
            matches!(
                resp,
                Response::Error {
                    code: ErrorCode::NotFound,
                    ..
                }
            ),
            "{resp:?}"
        );
        assert!(!state.config_path.exists());
    }
}
