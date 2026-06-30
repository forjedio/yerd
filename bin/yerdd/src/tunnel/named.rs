//! Named Tunnel (Phase 2) handlers: Cloudflare account login, tunnel
//! create/list/route, per-site hostname persistence, and named-tunnel start.
//!
//! Account-mutating `cloudflared` subcommands (login/create/route) run as
//! one-shot children with `HOME` and the origin cert pinned under
//! `{data}/tunnel`, so `cloudflared` never touches `~/.cloudflared` and every
//! secret stays in the daemon-owned 0700 dir. The long-running `tunnel run` goes
//! through the shared [`super::run_to_ready`] (it needs only `--config` +
//! `--origincert`, no env).
//!
//! Model (v1): one Cloudflare login and a single named tunnel (creating a second
//! is rejected), plus a per-site `site → hostname` mapping. `StartNamedTunnel`
//! renders one consolidated `config.yml` with an ingress rule per enabled site
//! and runs the single tunnel that serves them all.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};

use yerd_ipc::{ErrorCode, NamedTunnelMeta, Response, SiteHostname};
use yerd_platform::PlatformDirs;
use yerd_tunnel::parse::{find_auth_url, find_tunnel_id};
use yerd_tunnel::{OriginTarget, TunnelKind};

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
const NAMED_KEY: &str = "@named";

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
/// capturing its output. Bounded by [`ONESHOT_TIMEOUT`].
async fn run_oneshot(
    dirs: &PlatformDirs,
    args: Vec<OsString>,
) -> Result<std::process::Output, String> {
    let binary = install::binary_path(dirs);
    let mut cmd = tokio::process::Command::new(&binary);
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
    if !install::is_installed(&state.dirs) {
        return not_installed();
    }
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
        let binary = install::binary_path(&state.dirs);
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
    if !install::is_installed(&state.dirs) {
        return not_installed();
    }
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
    let out = match run_oneshot(&state.dirs, args).await {
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
    let (tunnels, sites) = {
        let cfg = state.config.lock().await;
        let tunnels = cfg
            .tunnel
            .named
            .iter()
            .map(|(name, uuid)| NamedTunnelMeta {
                name: name.clone(),
                uuid: uuid.clone(),
            })
            .collect();
        let sites = cfg
            .tunnel
            .sites
            .iter()
            .map(|(site, hostname)| SiteHostname {
                site: site.clone(),
                hostname: hostname.clone(),
            })
            .collect();
        (tunnels, sites)
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
    if !install::is_installed(&state.dirs) {
        return not_installed();
    }
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
    match run_oneshot(&state.dirs, args).await {
        Ok(out) if out.status.success() => Response::Ok,
        Ok(out) => internal(format!("route dns failed: {}", last_error_line(&out))),
        Err(e) => internal(e),
    }
}

/// `SetSiteTunnel` - persist (or clear, with `None`) a site's public hostname.
///
/// Setting a hostname is rejected for a site that does not exist, so the
/// persisted map can never accumulate entries that `list` would show but
/// `start` silently drops. Clearing (`None`) is always allowed so a stale entry
/// for a since-removed site can still be cleaned up.
pub async fn set_site_hostname(
    site: &str,
    hostname: Option<&str>,
    state: &DaemonState,
) -> Response {
    if hostname.is_some() && super::resolve_site(state, site).await.is_none() {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("no site named {site:?}"),
        };
    }
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    match hostname {
        Some(h) => {
            new.tunnel.sites.insert(site.to_owned(), h.to_owned());
        }
        None => {
            new.tunnel.sites.remove(site);
        }
    }
    if let Err(e) = new.validate() {
        return internal(format!("config validation failed: {e}"));
    }
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *cfg_guard = new;
    Response::Ok
}

/// `StartNamedTunnel` - (re)start the single consolidated named tunnel serving
/// every enabled site. Builds one `config.yml` with one ingress rule per enabled
/// site (resolving each site's local origin), stops any running named process so
/// the new config takes effect, then runs it through the shared tick driver.
pub async fn start(state: &DaemonState) -> Response {
    if !install::is_installed(&state.dirs) {
        return not_installed();
    }
    if !is_logged_in(&state.dirs) {
        return need_login();
    }

    let _guard = state.tunnel_mutate.lock().await;

    let (uuid, tunnel_name, enabled) = {
        let cfg = state.config.lock().await;
        let Some((tunnel_name, uuid)) = cfg
            .tunnel
            .named
            .iter()
            .next()
            .map(|(n, u)| (n.clone(), u.clone()))
        else {
            return Response::Error {
                code: ErrorCode::NotFound,
                message: "no named tunnel created yet - create one first".into(),
            };
        };
        let enabled: Vec<(String, String)> = cfg
            .tunnel
            .sites
            .iter()
            .map(|(s, h)| (s.clone(), h.clone()))
            .collect();
        (uuid, tunnel_name, enabled)
    };

    if enabled.is_empty() {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: "no sites enabled - set a hostname for a site first".into(),
        };
    }

    let mut rules = Vec::new();
    for (site, hostname) in &enabled {
        if let Some((name, secure, tld)) = super::resolve_site(state, site).await {
            let origin =
                OriginTarget::for_site(&name, &tld, secure, state.http.bound, state.https.bound);
            rules.push(yerd_tunnel::IngressRule {
                hostname: hostname.clone(),
                origin,
            });
        }
    }
    if rules.is_empty() {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: "no enabled sites currently exist".into(),
        };
    }

    let creds = creds_file(&state.dirs, &tunnel_name);
    let config_yml = yerd_tunnel::config::render_ingress_config(&uuid, &creds, &rules);
    let config_path = install::tunnel_dir(&state.dirs).join("named.yml");
    if let Err(e) = ensure_secret_dirs(&state.dirs) {
        return internal(e);
    }
    if let Err(e) = yerd_php::io::atomic_write::write(&config_path, config_yml.as_bytes()) {
        return internal(format!("write tunnel config: {e}"));
    }

    if let Err(e) = state.tunnel_manager.lock().await.stop(NAMED_KEY).await {
        return internal(format!(
            "could not stop the running named tunnel to apply the new config: {e}"
        ));
    }
    let args = yerd_tunnel::args::named_run_args(&config_path, &origincert(&state.dirs));
    let mut env = super::pinned_home_env(state);
    env.push((
        "TUNNEL_ORIGIN_CERT".into(),
        origincert(&state.dirs).into_os_string(),
    ));
    super::run_to_ready(state, NAMED_KEY, args, env, TunnelKind::Named, None).await
}

/// `StopNamedTunnel` - tear down the consolidated named tunnel.
pub async fn stop(state: &DaemonState) -> Response {
    {
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
    if !install::is_installed(&state.dirs) {
        return not_installed();
    }
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

    if let Err(e) = state.tunnel_manager.lock().await.stop(NAMED_KEY).await {
        return internal(format!(
            "could not stop the named tunnel before deleting it: {e}"
        ));
    }

    let cert = origincert(&state.dirs);
    let _ = run_oneshot(&state.dirs, yerd_tunnel::args::cleanup_args(name, &cert)).await;
    match run_oneshot(&state.dirs, yerd_tunnel::args::delete_args(name, &cert)).await {
        Ok(out) if out.status.success() => {}
        Ok(out) => return internal(format!("delete tunnel failed: {}", last_error_line(&out))),
        Err(e) => return internal(e),
    }

    let _ = std::fs::remove_file(creds_file(&state.dirs, name));

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
    tracing::info!(name, "deleted named tunnel");
    Response::Ok
}
