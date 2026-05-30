//! IPC accept loop + per-request dispatch.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use interprocess::local_socket::tokio::Listener;
use interprocess::local_socket::tokio::Stream as IpcStream;
use interprocess::local_socket::traits::tokio::Listener as _;
use interprocess::local_socket::traits::tokio::Stream as _;
use tokio::sync::watch;

use yerd_ipc::{
    read_message, write_message, ErrorCode, FrameDecoder, IpcError, Request, Response,
    DEFAULT_MAX_FRAME,
};
use yerd_php::Downloader; // brings the `download` method into scope for `update_php`

use crate::error::DaemonError;
use crate::state::DaemonState;
use crate::{mutate, startup};

/// Run the IPC accept loop until `shutdown_rx` resolves.
pub async fn run(
    listener: Listener,
    state: Arc<DaemonState>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok(stream) => {
                        let state = state.clone();
                        tokio::spawn(handle_client(stream, state));
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "ipc accept failed");
                    }
                }
            }
        }
    }
}

async fn handle_client(stream: IpcStream, state: Arc<DaemonState>) {
    let (reader, writer) = stream.split();
    let mut reader = reader;
    let mut writer = writer;
    let mut decoder = FrameDecoder::new();
    loop {
        let req = match read_message::<_, Request>(&mut reader, &mut decoder).await {
            Ok(Some(r)) => r,
            Ok(None) => return,
            Err(e) => {
                // Decode errors close the connection but stay quiet at
                // debug — common with mismatched-version clients.
                if !matches!(e, IpcError::UnexpectedEof { .. }) {
                    tracing::debug!(error = %e, "ipc decode error");
                }
                return;
            }
        };
        let resp = dispatch(req, &state).await;
        if let Err(e) = write_message(&mut writer, &resp, DEFAULT_MAX_FRAME).await {
            tracing::debug!(error = %e, "ipc write error");
            return;
        }
    }
}

async fn dispatch(req: Request, state: &DaemonState) -> Response {
    match req {
        Request::Ping => Response::Pong,
        Request::ListSites => Response::Sites {
            sites: state.router.read().await.iter().cloned().collect(),
        },
        Request::DaemonInfo => Response::Info {
            dns_addr: state.dns_addr,
            tld: state.config.lock().await.tld.as_str().to_owned(),
            ca_path: state.ca_path.clone(),
            ca_fingerprint: state.ca_fingerprint.to_hex(),
        },
        Request::Park { .. }
        | Request::Link { .. }
        | Request::Unlink { .. }
        | Request::SetPhp { .. }
        | Request::SetSecure { .. } => handle_mutation(req, state).await,
        Request::ListPhp => php_versions_response(state).await,
        Request::InstallPhp { version } => install_php(version, state).await,
        Request::SetDefaultPhp { version } => set_default_php(version, state).await,
        Request::CheckPhpUpdates => {
            let dl = crate::php_install::ReqwestDownloader::new();
            crate::php_updates::poll_and_refresh(state, &dl).await;
            php_versions_response(state).await
        }
        Request::UpdatePhp { version } => update_php(version, state).await,
        Request::AvailablePhp => available_php_response(state).await,
        Request::Status => Response::Status {
            report: Box::new(build_status_report(state).await),
        },
        Request::Diagnose => Response::Diagnoses {
            items: yerd_doctor::diagnose(&build_status_report(state).await),
        },
        Request::DoctorFix => run_doctor_fix(state).await,
        // `Request` is `#[non_exhaustive]` (external crate): a wildcard is
        // required even though every known variant is handled above.
        _ => Response::Error {
            code: ErrorCode::Internal,
            message: "unsupported request".into(),
        },
    }
}

/// Installed PHP versions — the bundled installs in yerd's data dir — ascending
/// and deduped. The single source of "what's installed" for the `PhpVersions`
/// and `AvailablePhp` replies.
fn installed_versions(state: &DaemonState) -> Vec<yerd_core::PhpVersion> {
    let mut installed: Vec<yerd_core::PhpVersion> = Vec::new();
    if let Ok(bundled) = yerd_php::discover_bundled(&state.dirs) {
        installed.extend(bundled.into_iter().map(|(v, _)| v));
    }
    installed.sort_unstable();
    installed.dedup();
    installed
}

/// Build the `PhpVersions` reply: installed versions, the live global default,
/// and cached update annotations. Read-only; no network.
async fn php_versions_response(state: &DaemonState) -> Response {
    Response::PhpVersions {
        installed: installed_versions(state),
        default: state.config.lock().await.php.default,
        updates: crate::php_updates::cached_updates(state).await,
    }
}

/// `available php` — list the major.minor versions installable from the
/// distribution, plus what's already installed (so clients hide or tag them).
/// Fetches the listing on demand; only a fetch/transport failure is an error
/// (an empty parse result is a valid empty list).
async fn available_php_response(state: &DaemonState) -> Response {
    let dl = crate::php_install::ReqwestDownloader::new();
    available_php_with(state, &dl).await
}

/// Injectable core of [`available_php_response`] (the downloader is a parameter
/// so tests can feed a fixture listing without touching the network).
async fn available_php_with(state: &DaemonState, dl: &dyn yerd_php::Downloader) -> Response {
    let (os, arch) = match yerd_php::current_os_arch() {
        Ok(p) => p,
        Err(e) => {
            return Response::Error {
                code: php_error_code(&e),
                message: e.to_string(),
            }
        }
    };
    let listing = match dl.download(&yerd_php::listing_url()).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => return internal(format!("couldn't reach the PHP distribution: {e}")),
    };
    Response::AvailablePhp {
        available: yerd_php::available_minors(&listing, os, arch),
        installed: installed_versions(state),
    }
}

/// Assemble a read-only [`yerd_ipc::StatusReport`].
///
/// Lock discipline: each guard is acquired, drained into owned data, and dropped
/// before the next acquisition — never two at once, never a guard held across an
/// `.await` that touches another lock. Mirrors the hazard documented in
/// `handle_mutation`.
#[allow(clippy::too_many_lines)] // straight-line assembly: one block per fact
async fn build_status_report(state: &DaemonState) -> yerd_ipc::StatusReport {
    use yerd_platform::SystemMetrics;

    // 1. Router read → site counts (guard dropped at block end).
    let sites = {
        let router = state.router.read().await;
        let mut counts = yerd_ipc::SiteCounts::default();
        for s in router.iter() {
            match s.kind() {
                yerd_core::SiteKind::Parked => counts.parked += 1,
                yerd_core::SiteKind::Linked => counts.linked += 1,
            }
            if s.secure() {
                counts.secured += 1;
            }
        }
        counts
    };

    // 2. Config lock → tld + default PHP (dropped).
    let (tld, default_php) = {
        let cfg = state.config.lock().await;
        (cfg.tld.as_str().to_owned(), cfg.php.default)
    };

    // 3. PHP manager lock → live pool snapshots (dropped).
    let snapshots = {
        let mut mgr = state.php_manager.lock().await;
        mgr.snapshots()
    };

    // 4. Installed versions + cached updates, off any guard.
    let installed = installed_versions(state);
    let updates = crate::php_updates::cached_updates(state).await;

    let metrics = yerd_platform::ActiveSystemMetrics::new();
    let php: Vec<yerd_ipc::PhpPoolStatus> = installed
        .iter()
        .map(|v| {
            let snap = snapshots.iter().find(|s| s.version == *v);
            let (run_state, pid, listen) = match snap {
                Some(s) => (
                    map_pool_state(s.state),
                    s.pid,
                    s.listen.as_ref().map(ToString::to_string),
                ),
                None => (yerd_ipc::PoolRunState::Stopped, None, None),
            };
            yerd_ipc::PhpPoolStatus {
                version: *v,
                installed_patch: crate::php_install::installed_patch(&state.dirs, *v),
                state: run_state,
                pid,
                listen,
                rss_bytes: pid.and_then(|p| metrics.rss_bytes(p)),
                update_available: updates
                    .iter()
                    .find(|u| u.version == *v)
                    .map(|u| u.latest.clone()),
            }
        })
        .collect();

    // 5. Unprivileged probes off any guard, on a blocking thread, errors → None.
    let fp = state.ca_fingerprint;
    let trusted_system = tokio::task::spawn_blocking(move || {
        use yerd_platform::TrustStore;
        yerd_platform::ActiveTrustStore::new()
            .is_present_system(&fp)
            .ok()
    })
    .await
    .ok()
    .flatten();

    let tld_probe = tld.clone();
    let resolver_installed = tokio::task::spawn_blocking(move || {
        use yerd_platform::ResolverInstaller;
        yerd_platform::ActiveResolverInstaller::new()
            .is_installed(&tld_probe)
            .ok()
    })
    .await
    .ok()
    .flatten();

    let load_avg = metrics
        .load_average()
        .map(|[a, b, c]| [load_to_centi(a), load_to_centi(b), load_to_centi(c)]);

    yerd_ipc::StatusReport {
        daemon_pid: std::process::id(),
        uptime_secs: state.started_at.elapsed().as_secs(),
        tld,
        http: state.http,
        https: state.https,
        dns_addr: state.dns_addr,
        ca: yerd_ipc::CaStatus {
            path: state.ca_path.clone(),
            fingerprint: state.ca_fingerprint.to_hex(),
            trusted_system,
        },
        resolver_installed,
        default_php,
        php,
        sites,
        load_avg,
    }
}

/// Map a `yerd-php` pool state to the wire enum.
fn map_pool_state(s: yerd_php::PoolRunState) -> yerd_ipc::PoolRunState {
    match s {
        yerd_php::PoolRunState::Running => yerd_ipc::PoolRunState::Running,
        yerd_php::PoolRunState::Failed => yerd_ipc::PoolRunState::Failed,
    }
}

/// Convert a (non-negative) load-average figure to integer hundredths, clamped
/// into `u32`. The `as` cast is sign- and range-safe given the explicit clamp.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn load_to_centi(x: f64) -> u32 {
    let v = (x * 100.0).round();
    if v <= 0.0 {
        0
    } else if v >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        v as u32
    }
}

/// `doctor fix` — run the safe auto-fixes, then re-diagnose for the remainder.
async fn run_doctor_fix(state: &DaemonState) -> Response {
    let report = build_status_report(state).await;
    let mut performed: Vec<yerd_ipc::FixResult> = Vec::new();

    for action in yerd_doctor::plan_auto_fixes(&report) {
        // `FixAction` is `#[non_exhaustive]`; `if let` handles the one known
        // variant and ignores any future ones safely.
        if let yerd_doctor::FixAction::RestartFpm(v) = action {
            let outcome = {
                let mut mgr = state.php_manager.lock().await;
                mgr.restart(v).await
            };
            performed.push(match outcome {
                Ok(_) => yerd_ipc::FixResult {
                    code: yerd_ipc::DiagnosisCode::FpmPoolFailed,
                    ok: true,
                    message: format!("restarted PHP {v} FPM pool"),
                },
                Err(e) => yerd_ipc::FixResult {
                    code: yerd_ipc::DiagnosisCode::FpmPoolFailed,
                    ok: false,
                    message: format!("failed to restart PHP {v}: {e}"),
                },
            });
        }
    }

    // Re-diagnose against a fresh report; surface the remaining problems.
    let after = build_status_report(state).await;
    let manual = yerd_doctor::diagnose(&after)
        .into_iter()
        .filter(|d| {
            matches!(
                d.severity,
                yerd_ipc::Severity::Warn | yerd_ipc::Severity::Fail
            )
        })
        .collect();

    Response::DoctorFix {
        report: yerd_ipc::FixReport { performed, manual },
    }
}

/// `update php [<ver>]` — upgrade the given minor (or all installed) to the
/// latest published patch when newer; refresh the cache; return the new list.
async fn update_php(version: Option<yerd_core::PhpVersion>, state: &DaemonState) -> Response {
    let dl = crate::php_install::ReqwestDownloader::new();
    let (os, arch) = match yerd_php::current_os_arch() {
        Ok(p) => p,
        Err(e) => {
            return Response::Error {
                code: php_error_code(&e),
                message: e.to_string(),
            }
        }
    };
    let targets: Vec<yerd_core::PhpVersion> = match version {
        Some(v) => {
            if crate::php_install::installed_patch(&state.dirs, v).is_none() {
                return Response::Error {
                    code: ErrorCode::NotFound,
                    message: format!("PHP {v} is not installed — run `yerd install php {v}`"),
                };
            }
            vec![v]
        }
        None => crate::php_updates::installed_minors(state),
    };
    let listing = match dl.download(&yerd_php::listing_url()).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => return internal(format!("listing fetch failed: {e}")),
    };
    for minor in targets {
        let Some(installed) = crate::php_install::installed_patch(&state.dirs, minor) else {
            continue;
        };
        let Ok(artifact) = yerd_php::resolve_from_listing(&listing, minor, os, arch) else {
            continue;
        };
        if yerd_php::is_newer(&installed, &artifact.full_version) {
            if let Err(e) = crate::php_install::install(minor, &state.dirs, &dl).await {
                return Response::Error {
                    code: php_error_code(&e),
                    message: e.to_string(),
                };
            }
            tracing::info!(version = %minor, from = %installed, to = %artifact.full_version, "updated PHP");
        }
    }
    crate::php_updates::poll_and_refresh(state, &dl).await;
    php_versions_response(state).await
}

/// `install php <ver>` — download + verify + unpack a prebuilt build. Runs the
/// (slow) download with no config lock held; the per-connection task model means
/// other clients are unaffected.
async fn install_php(version: yerd_core::PhpVersion, state: &DaemonState) -> Response {
    let dl = crate::php_install::ReqwestDownloader::new();
    match crate::php_install::install(version, &state.dirs, &dl).await {
        Ok(()) => {
            // The PhpManager's binary map is a startup snapshot; teach it about
            // the just-installed version so the proxy can spawn its FPM pool
            // without a daemon restart.
            refresh_php_binaries(state).await;
            Response::Ok
        }
        Err(e) => Response::Error {
            code: php_error_code(&e),
            message: e.to_string(),
        },
    }
}

/// Re-discover installed PHP binaries (bundled) and hand the refreshed map to
/// the live `PhpManager`. Mirrors the discovery done at startup.
async fn refresh_php_binaries(state: &DaemonState) {
    let binaries: std::collections::BTreeMap<yerd_core::PhpVersion, std::path::PathBuf> =
        match yerd_php::discover_bundled(&state.dirs) {
            Ok(b) => b.into_iter().collect(),
            Err(e) => {
                tracing::warn!(error = %e, "bundled PHP re-discovery failed after install");
                return;
            }
        };
    state.php_manager.lock().await.set_binaries(binaries);
}

/// `use <ver>` (global) — require the version installed, set the live default +
/// site fallback (`config.php.default`), persist, and repoint the `php` shim.
async fn set_default_php(version: yerd_core::PhpVersion, state: &DaemonState) -> Response {
    if !crate::php_install::cli_binary_path(&state.dirs, version).exists() {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("PHP {version} is not installed — run `yerd install php {version}`"),
        };
    }
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    new.php.default = version;
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    if let Err(e) = crate::php_install::set_default_shim(&state.dirs, version) {
        return internal(format!("update php shim failed: {e}"));
    }
    *cfg_guard = new;
    tracing::info!(version = %version, "set default PHP");
    Response::Ok
}

/// Map a [`yerd_php::PhpError`] to a wire [`ErrorCode`].
fn php_error_code(e: &yerd_php::PhpError) -> ErrorCode {
    use yerd_php::PhpError;
    match e {
        PhpError::UnsupportedPlatform { .. } | PhpError::VersionUnavailable { .. } => {
            ErrorCode::InvalidPath
        }
        _ => ErrorCode::Internal,
    }
}

/// Apply a mutation: canonicalise paths, run the pure delta, validate, persist,
/// and swap the live router — **build-then-validate-then-commit** so a failed
/// mutation leaves disk and the live router untouched.
async fn handle_mutation(req: Request, state: &DaemonState) -> Response {
    // 1. Canonicalise the path (Park/Link) *outside* the lock.
    let canonical = match &req {
        Request::Park { path } | Request::Link { path, .. } => match canonicalize_dir(path) {
            Ok(p) => Some(p),
            Err(resp) => return resp,
        },
        _ => None,
    };

    // 2. Take the config mutex for the whole build→commit (serializes writers).
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();

    // 3. Pure delta, reading the *pre-mutation* router so SetPhp promotion can
    //    recover a parked site's document_root. The read guard is an inline
    //    temporary dropped at the `;` — it must NOT be hoisted to a `let` and
    //    held across the step-7 write (that self-deadlocks the RwLock).
    // Source the linked-site default from the *live* config (not the startup
    // snapshot) so `SetDefaultPhp` (`yerd use <ver>`) changes the fallback that
    // newly-linked/promoted sites inherit.
    let live_default = new.php.default;
    let applied = match mutate::apply(
        &mut new,
        &*state.router.read().await,
        &req,
        canonical,
        live_default,
    ) {
        Ok(a) => a,
        Err(e) => {
            return Response::Error {
                code: mutate::error_code(&e),
                message: e.to_string(),
            }
        }
    };

    // 4. Never persist an invalid config.
    if let Err(e) = new.validate() {
        return internal(format!("config validation failed: {e}"));
    }

    // 5. Build the candidate router (re-scans parked roots).
    let candidate = match startup::build_router(&new, &state.dirs) {
        Ok(r) => r,
        Err(DaemonError::Core(yerd_core::CoreError::DuplicateSite { name })) => {
            return Response::Error {
                code: ErrorCode::AlreadyExists,
                message: format!("duplicate site: {name}"),
            }
        }
        Err(e) => return internal(format!("router rebuild failed: {e}")),
    };

    // 6. Persist atomically (write-temp-then-rename).
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }

    // 7. Commit: swap in the new config + router.
    *cfg_guard = new;
    *state.router.write().await = candidate;
    drop(cfg_guard);

    tracing::info!(summary = %applied.summary, "applied mutation");
    Response::Ok
}

/// Canonicalise `path` and require it to be an existing directory, or return a
/// ready-made `InvalidPath` error response.
fn canonicalize_dir(path: &Path) -> Result<PathBuf, Response> {
    match std::fs::canonicalize(path) {
        Ok(p) if p.is_dir() => Ok(p),
        Ok(p) => Err(invalid_path(format!("not a directory: {}", p.display()))),
        Err(e) => Err(invalid_path(format!(
            "cannot resolve {}: {e}",
            path.display()
        ))),
    }
}

fn invalid_path(message: String) -> Response {
    Response::Error {
        code: ErrorCode::InvalidPath,
        message,
    }
}

fn internal(message: String) -> Response {
    tracing::warn!(%message, "mutation failed");
    Response::Error {
        code: ErrorCode::Internal,
        message,
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
    use tokio::sync::{Mutex, RwLock};
    use yerd_core::{PhpVersion, RouterConfig, SiteRouter, Tld};
    use yerd_platform::PlatformDirs;

    fn dirs_in(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    fn state_in(tmp: &Path) -> DaemonState {
        let dirs = dirs_in(tmp);
        let router = SiteRouter::new(RouterConfig::with_tld(Tld::new("test").unwrap()));
        let ca_path = dirs.data.join("ca.cert.pem");
        let php_manager = std::sync::Arc::new(Mutex::new(yerd_php::PhpManager::new(
            yerd_php::TokioProcessSpawner,
            yerd_php::SystemClock,
            yerd_php::io::FastCgiProbe,
            dirs.clone(),
            yerd_platform::ActivePortBinder::new(),
            std::process::id(),
            std::collections::BTreeMap::new(),
        )));
        DaemonState {
            config: Mutex::new(yerd_config::Config::default()),
            router: Arc::new(RwLock::new(router)),
            config_path: dirs.config.join("yerd.toml"),
            dirs,
            dns_addr: "127.0.0.1:1053".parse().unwrap(),
            ca_path,
            ca_fingerprint: yerd_platform::CaFingerprint::new([0u8; 32]),
            php_updates: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            php_manager,
            http: yerd_ipc::PortStatus {
                requested: 80,
                bound: 8080,
                fell_back: true,
            },
            https: yerd_ipc::PortStatus {
                requested: 443,
                bound: 8443,
                fell_back: true,
            },
            started_at: std::time::Instant::now(),
        }
    }

    #[tokio::test]
    async fn dispatch_ping_returns_pong() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert!(matches!(
            dispatch(Request::Ping, &state).await,
            Response::Pong
        ));
    }

    #[tokio::test]
    async fn dispatch_list_sites_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match dispatch(Request::ListSites, &state).await {
            Response::Sites { sites } => assert!(sites.is_empty()),
            other => panic!("expected Sites, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn park_lists_child_and_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let sites_root = tmp.path().join("sites");
        std::fs::create_dir_all(sites_root.join("blog")).unwrap();
        let state = state_in(tmp.path());

        let resp = dispatch(
            Request::Park {
                path: sites_root.clone(),
            },
            &state,
        )
        .await;
        assert!(matches!(resp, Response::Ok), "got {resp:?}");

        // The child directory is the routable site, not the root.
        match dispatch(Request::ListSites, &state).await {
            Response::Sites { sites } => {
                let names: Vec<&str> = sites.iter().map(yerd_core::Site::name).collect();
                assert_eq!(names, vec!["blog"]);
            }
            other => panic!("expected Sites, got {other:?}"),
        }
        // Config persisted to disk + reflected in memory.
        assert!(state.config_path.exists());
        assert!(!state.config.lock().await.parked.paths.is_empty());
    }

    #[tokio::test]
    async fn link_then_duplicate_is_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let docroot = tmp.path().join("foo");
        std::fs::create_dir_all(&docroot).unwrap();
        let state = state_in(tmp.path());

        let ok = dispatch(
            Request::Link {
                name: "foo".into(),
                path: docroot.clone(),
            },
            &state,
        )
        .await;
        assert!(matches!(ok, Response::Ok), "got {ok:?}");

        let dup = dispatch(
            Request::Link {
                name: "foo".into(),
                path: docroot,
            },
            &state,
        )
        .await;
        match dup {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::AlreadyExists),
            other => panic!("expected AlreadyExists error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn park_nonexistent_is_invalid_path() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match dispatch(
            Request::Park {
                path: tmp.path().join("does-not-exist"),
            },
            &state,
        )
        .await
        {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::InvalidPath),
            other => panic!("expected InvalidPath, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unlink_unknown_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match dispatch(
            Request::Unlink {
                name: "ghost".into(),
            },
            &state,
        )
        .await
        {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::NotFound),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn use_promotes_parked_site_mixed_case() {
        let tmp = tempfile::tempdir().unwrap();
        let sites_root = tmp.path().join("sites");
        std::fs::create_dir_all(sites_root.join("blog")).unwrap();
        let state = state_in(tmp.path());
        dispatch(Request::Park { path: sites_root }, &state).await;

        // Mixed-case name must resolve the stored lowercase `blog`.
        let resp = dispatch(
            Request::SetPhp {
                name: "Blog".into(),
                version: PhpVersion::new(8, 4),
            },
            &state,
        )
        .await;
        assert!(matches!(resp, Response::Ok), "got {resp:?}");

        match dispatch(Request::ListSites, &state).await {
            Response::Sites { sites } => {
                let blog = sites.iter().find(|s| s.name() == "blog").unwrap();
                assert_eq!(blog.php(), PhpVersion::new(8, 4));
                assert_eq!(blog.kind(), yerd_core::SiteKind::Linked);
            }
            other => panic!("expected Sites, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_secure_promotes_parked_and_flips_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let sites_root = tmp.path().join("sites");
        std::fs::create_dir_all(sites_root.join("blog")).unwrap();
        let state = state_in(tmp.path());
        dispatch(Request::Park { path: sites_root }, &state).await;

        // Securing a parked site (mixed-case) promotes it and sets the flag.
        let resp = dispatch(
            Request::SetSecure {
                name: "Blog".into(),
                secure: true,
            },
            &state,
        )
        .await;
        assert!(matches!(resp, Response::Ok), "got {resp:?}");

        match dispatch(Request::ListSites, &state).await {
            Response::Sites { sites } => {
                let blog = sites.iter().find(|s| s.name() == "blog").unwrap();
                assert!(blog.secure());
                assert_eq!(blog.kind(), yerd_core::SiteKind::Linked);
            }
            other => panic!("expected Sites, got {other:?}"),
        }

        // Unsecuring flips it back.
        let resp = dispatch(
            Request::SetSecure {
                name: "blog".into(),
                secure: false,
            },
            &state,
        )
        .await;
        assert!(matches!(resp, Response::Ok), "got {resp:?}");
        match dispatch(Request::ListSites, &state).await {
            Response::Sites { sites } => {
                let blog = sites.iter().find(|s| s.name() == "blog").unwrap();
                assert!(!blog.secure());
            }
            other => panic!("expected Sites, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_daemon_info_reports_runtime_facts() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match dispatch(Request::DaemonInfo, &state).await {
            Response::Info {
                dns_addr,
                tld,
                ca_path,
                ca_fingerprint,
            } => {
                assert_eq!(dns_addr, state.dns_addr);
                assert_eq!(tld, "test");
                assert_eq!(ca_path, state.ca_path);
                // 64 lowercase hex chars; matches the stored fingerprint.
                assert_eq!(ca_fingerprint, state.ca_fingerprint.to_hex());
                assert_eq!(ca_fingerprint.len(), 64);
            }
            other => panic!("expected Info, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_status_reports_runtime_facts() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match dispatch(Request::Status, &state).await {
            Response::Status { report } => {
                assert_eq!(report.tld, "test");
                assert_eq!(report.default_php, PhpVersion::new(8, 3));
                assert_eq!(report.daemon_pid, std::process::id());
                // state_in seeds the rootless fallback ports.
                assert!(report.http.fell_back);
                assert_eq!(report.http.requested, 80);
                assert_eq!(report.http.bound, 8080);
                // No PHP installed under the tempdir.
                assert!(report.php.is_empty());
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_diagnose_flags_missing_php() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match dispatch(Request::Diagnose, &state).await {
            Response::Diagnoses { items } => {
                assert!(items
                    .iter()
                    .any(|d| d.code == yerd_ipc::DiagnosisCode::NoPhpInstalled));
            }
            other => panic!("expected Diagnoses, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_doctor_fix_with_no_pools_is_noop_but_reports_manual() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match dispatch(Request::DoctorFix, &state).await {
            Response::DoctorFix { report } => {
                // No running pools means nothing to auto-fix.
                assert!(report.performed.is_empty());
                // The unresolved problems (no PHP installed) surface as manual.
                assert!(report
                    .manual
                    .iter()
                    .any(|d| d.severity == yerd_ipc::Severity::Fail));
            }
            other => panic!("expected DoctorFix, got {other:?}"),
        }
    }

    /// Lay down a fake installed version: `data/php/php-<v>/{sbin/php-fpm,bin/php}`.
    fn fake_install(dirs: &PlatformDirs, v: PhpVersion) {
        fake_install_patch(dirs, v, &format!("{}.{}.0", v.major, v.minor));
    }

    /// Like `fake_install` but records a specific installed patch in the marker.
    fn fake_install_patch(dirs: &PlatformDirs, v: PhpVersion, full: &str) {
        let base = dirs
            .data
            .join("php")
            .join(format!("php-{}.{}", v.major, v.minor));
        std::fs::create_dir_all(base.join("sbin")).unwrap();
        std::fs::create_dir_all(base.join("bin")).unwrap();
        std::fs::write(base.join("sbin").join("php-fpm"), b"x").unwrap();
        std::fs::write(base.join("bin").join("php"), b"x").unwrap();
        std::fs::write(base.join(".yerd-version"), full).unwrap();
    }

    #[tokio::test]
    async fn dispatch_list_php_reports_installed_and_default() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, PhpVersion::new(8, 4));
        match dispatch(Request::ListPhp, &state).await {
            Response::PhpVersions {
                installed, default, ..
            } => {
                assert!(
                    installed.contains(&PhpVersion::new(8, 4)),
                    "got {installed:?}"
                );
                assert_eq!(default, PhpVersion::new(8, 3)); // Config::default()
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_set_default_php_requires_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match dispatch(
            Request::SetDefaultPhp {
                version: PhpVersion::new(8, 5),
            },
            &state,
        )
        .await
        {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::NotFound),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_set_default_php_sets_config_and_shim() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, PhpVersion::new(8, 4));
        let resp = dispatch(
            Request::SetDefaultPhp {
                version: PhpVersion::new(8, 4),
            },
            &state,
        )
        .await;
        assert!(matches!(resp, Response::Ok), "got {resp:?}");
        assert_eq!(state.config.lock().await.php.default, PhpVersion::new(8, 4));
        // The shim symlink now exists and points at the 8.4 CLI binary.
        let shim = state.dirs.data.join("bin").join("php");
        assert_eq!(
            std::fs::canonicalize(shim).unwrap(),
            std::fs::canonicalize(crate::php_install::cli_binary_path(
                &state.dirs,
                PhpVersion::new(8, 4)
            ))
            .unwrap()
        );
    }

    /// Guards the live-default fix: after `SetDefaultPhp`, a newly-linked site
    /// inherits the *new* default (not the startup snapshot).
    #[tokio::test]
    async fn set_default_php_changes_fallback_for_new_sites() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, PhpVersion::new(8, 4));
        let app_dir = tmp.path().join("app");
        std::fs::create_dir_all(&app_dir).unwrap();

        assert!(matches!(
            dispatch(
                Request::SetDefaultPhp {
                    version: PhpVersion::new(8, 4)
                },
                &state
            )
            .await,
            Response::Ok
        ));
        assert!(matches!(
            dispatch(
                Request::Link {
                    name: "app".into(),
                    path: app_dir,
                },
                &state
            )
            .await,
            Response::Ok
        ));
        match dispatch(Request::ListSites, &state).await {
            Response::Sites { sites } => {
                let app = sites.iter().find(|s| s.name() == "app").unwrap();
                assert_eq!(app.php(), PhpVersion::new(8, 4));
            }
            other => panic!("expected Sites, got {other:?}"),
        }
    }

    /// `ListPhp` annotates an installed minor from the (pre-seeded) update cache,
    /// with no network.
    #[tokio::test]
    async fn dispatch_list_php_surfaces_cached_update() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_patch(&state.dirs, PhpVersion::new(8, 5), "8.5.6");
        // Seed the cache as if a poll found a newer patch.
        state
            .php_updates
            .write()
            .await
            .insert(PhpVersion::new(8, 5), "8.5.7".to_owned());

        match dispatch(Request::ListPhp, &state).await {
            Response::PhpVersions { updates, .. } => {
                assert_eq!(updates.len(), 1);
                assert_eq!(updates[0].version, PhpVersion::new(8, 5));
                assert_eq!(updates[0].installed, "8.5.6");
                assert_eq!(updates[0].latest, "8.5.7");
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }
    }

    /// No cache entry (or not-newer) → no update annotation.
    #[tokio::test]
    async fn dispatch_list_php_no_update_when_cache_not_newer() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_patch(&state.dirs, PhpVersion::new(8, 5), "8.5.6");
        state
            .php_updates
            .write()
            .await
            .insert(PhpVersion::new(8, 5), "8.5.6".to_owned()); // same patch

        match dispatch(Request::ListPhp, &state).await {
            Response::PhpVersions { updates, .. } => assert!(updates.is_empty()),
            other => panic!("expected PhpVersions, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_update_php_unknown_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match dispatch(
            Request::UpdatePhp {
                version: Some(PhpVersion::new(8, 5)),
            },
            &state,
        )
        .await
        {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::NotFound),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    /// Fake downloader: directory URL (ends `/`) → the given listing; anything
    /// else errors (the poll only fetches the listing).
    struct ListingDl(String);
    #[async_trait::async_trait]
    impl yerd_php::Downloader for ListingDl {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            if url.ends_with('/') {
                Ok(self.0.clone().into_bytes())
            } else {
                Err(yerd_php::DownloadError::Transport {
                    url: url.to_owned(),
                    reason: "unexpected".into(),
                })
            }
        }
    }

    struct FailingDl;
    #[async_trait::async_trait]
    impl yerd_php::Downloader for FailingDl {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            Err(yerd_php::DownloadError::Transport {
                url: url.to_owned(),
                reason: "boom".into(),
            })
        }
    }

    #[tokio::test]
    async fn poll_and_refresh_populates_cache_from_listing() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_patch(&state.dirs, PhpVersion::new(8, 5), "8.5.6");
        let (os, arch) = yerd_php::current_os_arch().unwrap();
        let listing = format!("php-8.5.9-cli-{}-{}.tar.gz", os.as_str(), arch.as_str());

        crate::php_updates::poll_and_refresh(&state, &ListingDl(listing)).await;

        assert_eq!(
            state
                .php_updates
                .read()
                .await
                .get(&PhpVersion::new(8, 5))
                .map(String::as_str),
            Some("8.5.9")
        );
    }

    #[tokio::test]
    async fn poll_and_refresh_is_failure_tolerant() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_patch(&state.dirs, PhpVersion::new(8, 5), "8.5.6");
        state
            .php_updates
            .write()
            .await
            .insert(PhpVersion::new(8, 5), "8.5.6".to_owned());

        // Network failure must not panic and must leave the cache untouched.
        crate::php_updates::poll_and_refresh(&state, &FailingDl).await;

        assert_eq!(
            state
                .php_updates
                .read()
                .await
                .get(&PhpVersion::new(8, 5))
                .map(String::as_str),
            Some("8.5.6")
        );
    }

    #[tokio::test]
    async fn available_php_lists_distribution_minors_and_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_patch(&state.dirs, PhpVersion::new(8, 5), "8.5.6");
        let (os, arch) = yerd_php::current_os_arch().unwrap();
        let listing = format!(
            "php-8.3.20-cli-{os}-{arch}.tar.gz php-8.5.9-cli-{os}-{arch}.tar.gz",
            os = os.as_str(),
            arch = arch.as_str()
        );

        match available_php_with(&state, &ListingDl(listing)).await {
            Response::AvailablePhp {
                available,
                installed,
            } => {
                assert_eq!(
                    available,
                    vec![PhpVersion::new(8, 3), PhpVersion::new(8, 5)]
                );
                assert_eq!(installed, vec![PhpVersion::new(8, 5)]);
            }
            other => panic!("expected AvailablePhp, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn available_php_errors_on_fetch_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        match available_php_with(&state, &FailingDl).await {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::Internal),
            other => panic!("expected Error, got {other:?}"),
        }
    }
}
