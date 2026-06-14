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
        // A `RestartDaemon` request armed the flag in `dispatch`. Now that the
        // `Ok` is written, flush it and *then* trip the shutdown broadcast, so
        // the response is on the wire before any task observes teardown (no
        // timing race / sleep). `main` re-execs after the graceful shutdown.
        if state
            .restart_requested
            .load(std::sync::atomic::Ordering::Acquire)
        {
            use tokio::io::AsyncWriteExt as _;
            let _ = writer.flush().await;
            let _ = state.shutdown_tx.send_replace(true);
            return;
        }
    }
}

// One arm per request variant — the match is naturally long and grows with the
// protocol; splitting it would only scatter the routing.
#[allow(clippy::too_many_lines)]
async fn dispatch(req: Request, state: &DaemonState) -> Response {
    match req {
        Request::Ping => Response::Pong,
        Request::ListSites => Response::Sites {
            sites: state.router.read().await.iter().cloned().collect(),
        },
        // Registered parked roots, incl. empty ones (which produce no sites and
        // so never appear in `ListSites`). `parked.paths` is a `BTreeSet`, so the
        // collected order is already lexicographic — no explicit sort.
        Request::ListParked => Response::Parked {
            paths: state
                .config
                .lock()
                .await
                .parked
                .paths
                .iter()
                .cloned()
                .collect(),
        },
        Request::DaemonInfo => Response::Info {
            dns_addr: state.dns_addr,
            tld: state.config.lock().await.tld.as_str().to_owned(),
            ca_path: state.ca_path.clone(),
            ca_fingerprint: state.ca_fingerprint.to_hex(),
            http_port: state.http.bound,
            https_port: state.https.bound,
        },
        Request::Park { .. }
        | Request::Link { .. }
        | Request::Unlink { .. }
        | Request::Unpark { .. }
        | Request::SetPhp { .. }
        | Request::SetSecure { .. }
        | Request::SetWebRoot { .. } => handle_mutation(req, state).await,
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
        Request::SetPhpSettings { settings } => set_php_settings(settings, state).await,
        Request::RestartPhp { version } => restart_php(version, state).await,
        Request::RestartAllPhp => restart_all_php(state).await,
        Request::UninstallPhp { version } => uninstall_php(version, state).await,
        Request::Status => Response::Status {
            report: Box::new(build_status_report(state).await),
        },
        Request::Diagnose => Response::Diagnoses {
            items: yerd_doctor::diagnose(&build_status_report(state).await),
        },
        Request::DoctorFix => run_doctor_fix(state).await,
        // Arm the restart flag; `handle_client` trips the shutdown broadcast
        // *after* writing this `Ok`, and `main` re-execs once teardown completes.
        #[cfg(unix)]
        Request::RestartDaemon => {
            state
                .restart_requested
                .store(true, std::sync::atomic::Ordering::Release);
            Response::Ok
        }
        #[cfg(not(unix))]
        Request::RestartDaemon => Response::Error {
            code: ErrorCode::Internal,
            message: "daemon restart is not supported on this platform".into(),
        },
        Request::ListServices => crate::services::list_services(state).await,
        Request::AvailableServices => {
            let dl = crate::php_install::ReqwestDownloader::new();
            crate::services::available_services(state, &dl).await
        }
        Request::InstallService { service, version } => {
            let dl = crate::php_install::ReqwestDownloader::new();
            crate::services::install_service(&service, &version, state, &dl).await
        }
        Request::UninstallService {
            service,
            version,
            purge,
        } => crate::services::uninstall_service(&service, &version, purge, state).await,
        Request::StartService { service } => crate::services::start_service(&service, state).await,
        Request::StopService { service } => crate::services::stop_service(&service, state).await,
        Request::RestartService { service } => {
            crate::services::restart_service(&service, state).await
        }
        Request::SetServicePort { service, port } => {
            crate::services::set_service_port(&service, port, state).await
        }
        Request::ServiceLogs { service, lines } => {
            crate::services::service_logs(&service, lines, state)
        }
        Request::CreateDatabase { service, name } => {
            crate::db_admin::create(&service, &name, state).await
        }
        Request::ListDatabases { service } => crate::db_admin::list(&service, state).await,
        Request::DropDatabase { service, name } => {
            crate::db_admin::drop(&service, &name, state).await
        }
        Request::BackupDatabase {
            service,
            name,
            path,
        } => crate::db_admin::backup(&service, &name, &path, state).await,
        Request::RestoreDatabase {
            service,
            name,
            path,
        } => crate::db_admin::restore(&service, &name, &path, state).await,
        Request::ChangeServiceVersion { service, version } => {
            let dl = crate::php_install::ReqwestDownloader::new();
            crate::services::change_service_version(&service, &version, state, &dl).await
        }
        Request::ListDumps { since_id } => crate::dump_server::list(state, since_id).await,
        Request::ClearDumps => crate::dump_server::clear(state).await,
        Request::DeleteDump { id } => crate::dump_server::delete(state, id).await,
        Request::SetDumpsEnabled { enabled } => {
            crate::dump_server::set_enabled(state, enabled).await
        }
        Request::SetDumpsPort { port } => crate::dump_server::set_port(state, port).await,
        Request::SetDumpFeature { feature, enabled } => {
            crate::dump_server::set_feature(state, feature, enabled).await
        }
        Request::SetDumpsPersist { persist } => {
            crate::dump_server::set_persist(state, persist).await
        }
        Request::DumpsStatus => crate::dump_server::status(state).await,
        Request::ListMails => Response::Mails {
            mails: state.mail_store.list().await,
        },
        Request::GetMail { id } => match state.mail_store.get(&id).await {
            Ok(Some(mail)) => Response::Mail {
                mail: Box::new(mail),
            },
            Ok(None) => Response::Error {
                code: ErrorCode::NotFound,
                message: format!("no captured mail with id {id}"),
            },
            Err(e) => internal(format!("mail read failed: {e}")),
        },
        Request::ClearMails => match state.mail_store.clear().await {
            Ok(()) => Response::Ok,
            Err(e) => internal(format!("mail clear failed: {e}")),
        },
        Request::DeleteMails { ids } => match state.mail_store.delete_many(&ids).await {
            Ok(()) => Response::Ok,
            Err(e) => internal(format!("mail delete failed: {e}")),
        },
        Request::SetMailPort { port } => set_mail_port(port, state).await,
        Request::SetMailEnabled { enabled } => set_mail_enabled(enabled, state).await,
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
/// cached update annotations, and the global ini settings. Read-only; no network.
async fn php_versions_response(state: &DaemonState) -> Response {
    let (default, settings) = {
        let cfg = state.config.lock().await;
        (cfg.php.default, cfg.php.settings.clone())
    };
    Response::PhpVersions {
        installed: installed_versions(state),
        default,
        updates: crate::php_updates::cached_updates(state).await,
        settings,
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
    let listing = match dl.download(&yerd_php::listing_url(os)).await {
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

    // 2. Config lock → tld + default PHP + the *configured* mail enabled/port
    // (dropped). Sourcing mail enabled/port from config (not the startup
    // snapshot) means `SetMailPort`/`SetMailEnabled` are reflected in `Status`
    // immediately, so the GUI can confirm a save; `listening` below still comes
    // from the runtime snapshot, so an enabled-but-not-yet-bound state (a change
    // pending the next restart) reads as `enabled && !listening`.
    let (tld, default_php, mail_enabled, mail_port) = {
        let cfg = state.config.lock().await;
        (
            cfg.tld.as_str().to_owned(),
            cfg.php.default,
            cfg.mail.enabled,
            cfg.mail.port,
        )
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
    // `is_trusted` is an effective-trust check (not mere presence): macOS shells
    // `security verify-cert`, Linux checks anchor-dir presence. `ca_path` is a
    // `PathBuf` (not `Copy`), so clone it into the closure alongside `fp`.
    let fp = state.ca_fingerprint;
    let ca_path = state.ca_path.clone();
    let trusted_system = tokio::task::spawn_blocking(move || {
        use yerd_platform::TrustStore;
        yerd_platform::ActiveTrustStore::new()
            .is_trusted(&ca_path, &fp)
            .ok()
    })
    .await
    .ok()
    .flatten();

    let tld_probe = tld.clone();
    let dns_addr = state.dns_addr;
    let resolver_installed = tokio::task::spawn_blocking(move || {
        use yerd_platform::ResolverInstaller;
        yerd_platform::ActiveResolverInstaller::new()
            .is_installed(&tld_probe, dns_addr)
            .ok()
    })
    .await
    .ok()
    .flatten();

    // Active probes (bounded TCP/HTTP connects, off the executor so they can't
    // stall status assembly):
    //  - `port_redirect`: is the privileged-port redirect carrying 80/443 to
    //    *this* proxy? `None` on Linux (binds directly after setcap).
    //  - `foreign_web_listener`: is a non-Yerd process squatting 80/443?
    //    Cross-platform; confirmed via the proxy's `Server:` marker.
    let (port_redirect, foreign_web_listener) = tokio::task::spawn_blocking(|| {
        use yerd_platform::PortRedirector;
        let r = yerd_platform::ActivePortRedirector::new();
        (r.is_active(), r.foreign_web_listener())
    })
    .await
    .unwrap_or((None, None));

    // macOS only: if installing the resolver replaced a pre-existing file, the
    // path of the most recent backup (within the last week) so `doctor` can
    // point at it. `None` on Linux / when nothing was replaced. Off the executor
    // (fs I/O), mirroring the probes above.
    let backup_tld = tld.clone();
    let resolver_backup = tokio::task::spawn_blocking(move || latest_resolver_backup(&backup_tld))
        .await
        .ok()
        .flatten();

    let load_avg = metrics
        .load_average()
        .map(|[a, b, c]| [load_to_centi(a), load_to_centi(b), load_to_centi(c)]);

    yerd_ipc::StatusReport {
        daemon_pid: std::process::id(),
        uptime_secs: state.started_at.elapsed().as_secs(),
        daemon_rss_bytes: metrics.rss_bytes(std::process::id()),
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
        port_redirect,
        foreign_web_listener,
        resolver_backup,
        default_php,
        php,
        sites,
        load_avg,
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        services: crate::services::service_statuses(state).await,
        mail: Some(yerd_ipc::MailStatus {
            enabled: mail_enabled,
            port: mail_port,
            listening: state.mail.listening,
            count: state.mail_store.count().await,
        }),
    }
}

/// The path of the most recent replaced-resolver backup for `tld`, if one was
/// saved within the last 7 days. macOS-only — the helper writes these when it
/// overwrites a pre-existing `/etc/resolver/<tld>`. The age bound keeps the
/// `doctor` finding a transient migration notice rather than permanent noise.
#[cfg(target_os = "macos")]
fn latest_resolver_backup(tld: &str) -> Option<String> {
    use yerd_platform::pure::resolver_file;
    const MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

    let dir = resolver_file::macos_backup_dir();
    let names: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    let latest = resolver_file::latest_backup(&names, tld)?;
    let secs = resolver_file::parse_backup_secs(latest, tld)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // saturating: a future-dated backup (clock moved back) reads as age 0 → shown.
    (now.saturating_sub(secs) <= MAX_AGE_SECS)
        .then(|| dir.join(latest).to_string_lossy().into_owned())
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::missing_const_for_fn)]
fn latest_resolver_backup(_tld: &str) -> Option<String> {
    None
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
    let listing = match dl.download(&yerd_php::listing_url(os)).await {
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

/// `restart php <ver>` — stop + ensure the version's FPM pool. Starts a stopped
/// pool too (the GUI greys "Restart" when idle; the CLI may use it to start one).
async fn restart_php(version: yerd_core::PhpVersion, state: &DaemonState) -> Response {
    let outcome = {
        let mut mgr = state.php_manager.lock().await;
        mgr.restart(version).await
    };
    match outcome {
        Ok(_) => Response::Ok,
        Err(yerd_php::PhpError::VersionNotInstalled { version }) => Response::Error {
            code: ErrorCode::NotFound,
            message: format!("PHP {version} is not installed"),
        },
        Err(e) => internal(format!("restart of PHP {version} failed: {e}")),
    }
}

/// `restart php` (no version) — restart every started pool (running or failed).
/// Best-effort: a per-pool failure is logged, not fatal. Idle/never-started
/// ondemand pools are left alone (they spawn fresh on the next request).
async fn restart_all_php(state: &DaemonState) -> Response {
    let mut mgr = state.php_manager.lock().await;
    for snap in mgr.snapshots() {
        if let Err(e) = mgr.restart(snap.version).await {
            tracing::warn!(version = %snap.version, error = %e, "failed to restart FPM pool");
        }
    }
    Response::Ok
}

/// `uninstall php <ver>` — remove an installed version after safety checks.
///
/// Blocked (→ `InvalidPath` with a human message) when the version is in use by
/// a site, is the last installed version while sites remain, or is the current
/// default while other versions are installed. The config/router guards are
/// dropped before the filesystem remove + manager ops (lock discipline); a
/// concurrent `SetPhp` to this version is a benign microsecond TOCTOU, accepted
/// the same way `set_default_php` accepts its read-then-act window.
async fn uninstall_php(version: yerd_core::PhpVersion, state: &DaemonState) -> Response {
    let installed = installed_versions(state);
    if !installed.contains(&version) {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("PHP {version} is not installed"),
        };
    }

    let default = state.config.lock().await.php.default;
    let (sites_using, total_sites) = {
        let router = state.router.read().await;
        let using: Vec<String> = router
            .iter()
            .filter(|s| s.php() == version)
            .map(|s| s.name().to_owned())
            .collect();
        (using, router.iter().count())
    };

    if !sites_using.is_empty() {
        return Response::Error {
            code: ErrorCode::InvalidPath,
            message: format!(
                "PHP {version} is assigned to site(s): {} — reassign them first",
                sites_using.join(", ")
            ),
        };
    }
    if installed.len() <= 1 && total_sites > 0 {
        return Response::Error {
            code: ErrorCode::InvalidPath,
            message: format!(
                "can't uninstall PHP {version}: it's the last installed version and sites still exist"
            ),
        };
    }
    if version == default && installed.len() > 1 {
        return Response::Error {
            code: ErrorCode::InvalidPath,
            message: format!("PHP {version} is the default — set another version as default first"),
        };
    }

    // Stop the pool before removing its files (clean socket teardown). On
    // Windows `remove_dir_all` would fail while php-fpm.exe runs — revisit with
    // the Windows port.
    let _ = state.php_manager.lock().await.stop(version).await;
    let version_dir = state
        .dirs
        .data
        .join("php")
        .join(format!("php-{}.{}", version.major, version.minor));
    if let Err(e) = std::fs::remove_dir_all(&version_dir) {
        return internal(format!("failed to remove PHP {version}: {e}"));
    }
    refresh_php_binaries(state).await;
    tracing::info!(version = %version, "uninstalled PHP");
    php_versions_response(state).await
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

/// Set the mail-capture SMTP port. Persisted to config; takes effect on the next
/// daemon start/restart (no hot rebind), matching `SetServicePort`. Modelled on
/// `set_php_settings` (clone → set → validate → save → commit under the config
/// mutex) so an invalid value (e.g. a zero port) is rejected by the config
/// validator rather than overloading an unrelated `ErrorCode`.
async fn set_mail_port(port: u16, state: &DaemonState) -> Response {
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    new.mail.port = port;
    if let Err(e) = new.validate() {
        return internal(format!("config validation failed: {e}"));
    }
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *cfg_guard = new;
    tracing::info!(port, "set mail port (effective on next restart)");
    Response::Ok
}

/// Enable or disable mail capture. Persisted to config; takes effect on the next
/// daemon start/restart.
async fn set_mail_enabled(enabled: bool, state: &DaemonState) -> Response {
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    new.mail.enabled = enabled;
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *cfg_guard = new;
    tracing::info!(enabled, "set mail enabled (effective on next restart)");
    Response::Ok
}

/// `set/unset php` — merge global PHP ini settings into the config and apply
/// them to every live FPM pool. An empty-string value removes a key (reset to
/// PHP's default).
///
/// Order (build → validate → save → commit → drop config guard → restart
/// pools): the config write is fail-closed; the per-pool restart is best-effort
/// and runs *after* the config guard is released, under a single `php_manager`
/// lock so no request can `ensure` a stale-config pool mid-update.
async fn set_php_settings(
    settings: std::collections::BTreeMap<String, String>,
    state: &DaemonState,
) -> Response {
    // Build the merged settings on a config clone, validating each entry.
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    for (key, value) in settings {
        if value.is_empty() {
            new.php.settings.remove(&key);
            continue;
        }
        if let Err(e) = yerd_core::php_settings::validate_value(&key, &value) {
            return Response::Error {
                code: ErrorCode::InvalidPath,
                message: e.to_string(),
            };
        }
        let canonical = yerd_core::php_settings::canonical_value(&key, &value);
        new.php.settings.insert(key, canonical);
    }

    // No-op: nothing changed → skip the disk write and the pool restarts.
    if new.php.settings == cfg_guard.php.settings {
        drop(cfg_guard);
        return php_versions_response(state).await;
    }

    if let Err(e) = new.validate() {
        return internal(format!("config validation failed: {e}"));
    }
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    let applied = settings_to_vec(&new.php.settings);
    *cfg_guard = new;
    drop(cfg_guard);

    // Apply to the live pools: update the manager's settings and restart every
    // started pool so the new directives take effect. Single lock span.
    {
        let mut mgr = state.php_manager.lock().await;
        mgr.set_ini_settings(applied);
        for snap in mgr.snapshots() {
            if let Err(e) = mgr.restart(snap.version).await {
                tracing::warn!(version = %snap.version, error = %e, "failed to restart FPM pool after settings change");
            }
        }
    }
    tracing::info!("applied global PHP settings");
    php_versions_response(state).await
}

/// A config settings map as sorted `(name, value)` pairs for the pool manager.
fn settings_to_vec(settings: &std::collections::BTreeMap<String, String>) -> Vec<(String, String)> {
    settings
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
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
    let applied = match &req {
        // SetWebRoot needs the target site's document_root (from the router /
        // linked list) and does filesystem I/O (canonicalise + containment, or
        // re-detect). Resolve it here rather than in the pure `mutate::apply`.
        // The router read guard is an inline temporary dropped before step 7's
        // write — same discipline as the `mutate::apply` call below.
        Request::SetWebRoot { name, path } => {
            match resolve_web_root_mutation(
                &mut new,
                &*state.router.read().await,
                name,
                path.as_deref(),
            ) {
                Ok(a) => a,
                Err(resp) => return resp,
            }
        }
        _ => match mutate::apply(
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
        },
    };

    // 4. Never persist an invalid config.
    if let Err(e) = new.validate() {
        return internal(format!("config validation failed: {e}"));
    }

    // 5. Build the candidate router (re-scans parked roots).
    let candidate = match startup::build_router(&new, &state.dirs, &state.detect_cache) {
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

    // Nudge the filesystem watcher to reconcile its watch set against the new
    // config (e.g. a newly-parked root to start watching, or a now-resolved
    // site to stop watching).
    state.watch_dirty.notify_one();

    tracing::info!(summary = %applied.summary, "applied mutation");
    Response::Ok
}

/// Resolve a `SetWebRoot` request against `new`, doing the filesystem I/O
/// (containment check, or re-detection) the pure `mutate::apply` can't. A
/// **linked** site stores the chosen subpath on its `Site`; a **parked** site
/// stores it in `overrides[doc_root].web_root`. `path = None` resets to
/// auto-detect: re-detect now for linked, clear the override for parked.
fn resolve_web_root_mutation(
    new: &mut yerd_config::Config,
    router: &yerd_core::SiteRouter,
    name: &str,
    path: Option<&str>,
) -> Result<mutate::Applied, Response> {
    let name_lc = name.to_ascii_lowercase();

    // Linked sites carry the subpath directly on the persisted `Site`.
    if let Some(site) = new.linked.iter_mut().find(|s| s.name() == name_lc) {
        let doc_root = site.document_root().to_path_buf();
        let rel = if let Some(p) = path {
            resolve_web_root_within(&doc_root, p)?
        } else {
            yerd_core::detect(&yerd_platform::gather_project_signals(&doc_root))
                .subpath
                .to_string_lossy()
                .into_owned()
        };
        site.set_web_subpath(&rel);
        return Ok(mutate::Applied {
            summary: web_root_summary(&name_lc, &rel),
        });
    }

    // Parked sites store the pin in `overrides`, keyed by document_root.
    if let Some(parked) = router.get(&name_lc) {
        let key = parked.document_root().to_string_lossy().into_owned();
        if let Some(p) = path {
            let rel = resolve_web_root_within(parked.document_root(), p)?;
            new.overrides.entry(key).or_default().web_root = Some(rel.clone());
            return Ok(mutate::Applied {
                summary: web_root_summary(&name_lc, &rel),
            });
        }
        // Clear the web_root override; drop the whole entry if it no longer
        // pins anything, to avoid leaving an empty override.
        if let Some(ov) = new.overrides.get_mut(&key) {
            ov.web_root = None;
            if ov.php.is_none() && ov.secure.is_none() {
                new.overrides.remove(&key);
            }
        }
        return Ok(mutate::Applied {
            summary: format!("{name_lc} web root reset to auto-detect"),
        });
    }

    Err(Response::Error {
        code: ErrorCode::NotFound,
        message: format!("no site named {name_lc}"),
    })
}

/// One-line summary for a web-root change.
fn web_root_summary(name: &str, rel: &str) -> String {
    if rel.is_empty() {
        format!("{name} now served from its project root")
    } else {
        format!("{name} now served from {rel}")
    }
}

/// Resolve a user-supplied served path against `doc_root` and return the
/// validated **relative** remainder (empty = serve the document root itself).
///
/// Rejects anything that escapes `doc_root`. Both sides are canonicalised
/// before comparison so a `\\?\` verbatim prefix from `fs::canonicalize` on
/// Windows doesn't spuriously fail the containment check against the
/// non-verbatim stored `document_root`.
fn resolve_web_root_within(doc_root: &Path, input: &str) -> Result<String, Response> {
    let candidate = {
        let p = Path::new(input);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            doc_root.join(p)
        }
    };
    let canon_candidate = std::fs::canonicalize(&candidate)
        .map_err(|e| invalid_path(format!("cannot resolve {}: {e}", candidate.display())))?;
    if !canon_candidate.is_dir() {
        return Err(invalid_path(format!(
            "served path is not a directory: {}",
            canon_candidate.display()
        )));
    }
    let canon_root = std::fs::canonicalize(doc_root)
        .map_err(|e| invalid_path(format!("cannot resolve {}: {e}", doc_root.display())))?;
    let rel = canon_candidate.strip_prefix(&canon_root).map_err(|_| {
        invalid_path(format!(
            "served path must be inside the site directory ({})",
            canon_root.display()
        ))
    })?;
    Ok(rel.to_string_lossy().into_owned())
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
            service_manager: std::sync::Arc::new(Mutex::new(crate::services::new_manager(
                dirs_in(tmp),
            ))),
            mail_store: std::sync::Arc::new(yerd_mail::Store::open(tmp.join("mail")).unwrap()),
            mail: crate::state::MailRuntime { listening: false },
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
            shutdown_tx: tokio::sync::watch::channel(false).0,
            restart_requested: std::sync::atomic::AtomicBool::new(false),
            detect_cache: std::sync::Arc::new(crate::detect_cache::DetectCache::new()),
            watch_dirty: tokio::sync::Notify::new(),
            dumps: std::sync::Arc::new(crate::dump_server::DumpStore::new()),
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

    const SAMPLE_EML: &[u8] = b"From: Example <hello@example.com>\r\n\
To: test@test.com\r\n\
Subject: Captured\r\n\r\nhi\r\n";

    #[tokio::test]
    async fn dispatch_list_mails_empty_then_populated_then_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        // Empty.
        match dispatch(Request::ListMails, &state).await {
            Response::Mails { mails } => assert!(mails.is_empty()),
            other => panic!("expected Mails, got {other:?}"),
        }

        // Capture one directly through the store (the SMTP path is covered in
        // yerd-mail) and list it.
        state.mail_store.append(SAMPLE_EML).await.unwrap();
        let id = match dispatch(Request::ListMails, &state).await {
            Response::Mails { mails } => {
                assert_eq!(mails.len(), 1);
                assert_eq!(mails[0].subject, "Captured");
                mails[0].id.clone()
            }
            other => panic!("expected Mails, got {other:?}"),
        };

        // Fetch the detail by id.
        match dispatch(Request::GetMail { id: id.clone() }, &state).await {
            Response::Mail { mail } => assert_eq!(mail.subject, "Captured"),
            other => panic!("expected Mail, got {other:?}"),
        }

        // Unknown id → NotFound.
        match dispatch(
            Request::GetMail {
                id: "999999".into(),
            },
            &state,
        )
        .await
        {
            Response::Error { code, .. } => assert!(matches!(code, ErrorCode::NotFound)),
            other => panic!("expected NotFound, got {other:?}"),
        }

        // Clear empties the store.
        assert!(matches!(
            dispatch(Request::ClearMails, &state).await,
            Response::Ok
        ));
        match dispatch(Request::ListMails, &state).await {
            Response::Mails { mails } => assert!(mails.is_empty()),
            other => panic!("expected empty Mails, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_status_includes_mail() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match dispatch(Request::Status, &state).await {
            Response::Status { report } => {
                let mail = report.mail.expect("status should carry mail");
                // `enabled`/`port` come from the (default) config; `listening` is
                // the runtime snapshot (false in this test harness).
                assert!(mail.enabled);
                assert_eq!(mail.port, yerd_config::DEFAULT_MAIL_PORT);
                assert!(!mail.listening);
                assert_eq!(mail.count, 0);
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_set_mail_port_persists_and_rejects_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        // A zero port is rejected by the config validator (mapped to Internal),
        // not by overloading a path/port code.
        match dispatch(Request::SetMailPort { port: 0 }, &state).await {
            Response::Error { code, .. } => assert!(matches!(code, ErrorCode::Internal)),
            other => panic!("expected Error, got {other:?}"),
        }

        assert!(matches!(
            dispatch(Request::SetMailPort { port: 3030 }, &state).await,
            Response::Ok
        ));
        assert_eq!(state.config.lock().await.mail.port, 3030);

        assert!(matches!(
            dispatch(Request::SetMailEnabled { enabled: true }, &state).await,
            Response::Ok
        ));
        assert!(state.config.lock().await.mail.enabled);

        // Persisted to disk.
        let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
        assert_eq!(reloaded.mail.port, 3030);
        assert!(reloaded.mail.enabled);
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
    async fn set_web_root_explicit_then_auto_on_linked_site() {
        let tmp = tempfile::tempdir().unwrap();
        let docroot = tmp.path().join("app");
        std::fs::create_dir_all(docroot.join("public")).unwrap();
        std::fs::write(docroot.join("artisan"), b"").unwrap();
        std::fs::write(docroot.join("public/index.php"), b"").unwrap();
        let state = state_in(tmp.path());

        dispatch(
            Request::Link {
                name: "app".into(),
                path: docroot.clone(),
            },
            &state,
        )
        .await;

        // Explicit pin to "public".
        let ok = dispatch(
            Request::SetWebRoot {
                name: "app".into(),
                path: Some("public".into()),
            },
            &state,
        )
        .await;
        assert!(matches!(ok, Response::Ok), "got {ok:?}");
        let subpath = web_subpath_of(&state, "app").await;
        assert_eq!(subpath, std::path::PathBuf::from("public"));

        // Reset to auto-detect: the Laravel layout re-detects "public".
        let ok = dispatch(
            Request::SetWebRoot {
                name: "app".into(),
                path: None,
            },
            &state,
        )
        .await;
        assert!(matches!(ok, Response::Ok), "got {ok:?}");
        assert_eq!(
            web_subpath_of(&state, "app").await,
            std::path::PathBuf::from("public")
        );
    }

    #[tokio::test]
    async fn set_web_root_outside_document_root_is_invalid_path() {
        let tmp = tempfile::tempdir().unwrap();
        let docroot = tmp.path().join("app");
        std::fs::create_dir_all(&docroot).unwrap();
        // A sibling directory that exists but is outside the document root.
        std::fs::create_dir_all(tmp.path().join("outside")).unwrap();
        let state = state_in(tmp.path());
        dispatch(
            Request::Link {
                name: "app".into(),
                path: docroot,
            },
            &state,
        )
        .await;

        let resp = dispatch(
            Request::SetWebRoot {
                name: "app".into(),
                path: Some(tmp.path().join("outside").to_string_lossy().into_owned()),
            },
            &state,
        )
        .await;
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::InvalidPath),
            other => panic!("expected InvalidPath, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_web_root_unknown_site_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let resp = dispatch(
            Request::SetWebRoot {
                name: "ghost".into(),
                path: None,
            },
            &state,
        )
        .await;
        match resp {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::NotFound),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    /// Helper: read a site's web_subpath via `ListSites`.
    async fn web_subpath_of(state: &DaemonState, name: &str) -> std::path::PathBuf {
        match dispatch(Request::ListSites, state).await {
            Response::Sites { sites } => sites
                .iter()
                .find(|s| s.name() == name)
                .unwrap_or_else(|| panic!("site {name} not found"))
                .web_subpath()
                .to_path_buf(),
            other => panic!("expected Sites, got {other:?}"),
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
    async fn use_overrides_parked_site_keeping_kind_mixed_case() {
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
                // Override applied, but the site stays parked (no promotion).
                assert_eq!(blog.kind(), yerd_core::SiteKind::Parked);
            }
            other => panic!("expected Sites, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_parked_and_unpark_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        // One populated root (yields a site) and one empty root (yields none).
        let populated = tmp.path().join("populated");
        std::fs::create_dir_all(populated.join("blog")).unwrap();
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let state = state_in(tmp.path());
        dispatch(Request::Park { path: populated }, &state).await;
        dispatch(Request::Park { path: empty }, &state).await;

        // Both roots are registered — the empty one included.
        let parked = match dispatch(Request::ListParked, &state).await {
            Response::Parked { paths } => paths,
            other => panic!("expected Parked, got {other:?}"),
        };
        assert_eq!(parked.len(), 2, "both roots registered: {parked:?}");
        // BTreeSet → already lexicographically sorted.
        let mut sorted = parked.clone();
        sorted.sort();
        assert_eq!(parked, sorted, "ListParked must be sorted");
        let populated_root = parked
            .iter()
            .find(|p| p.ends_with("populated"))
            .unwrap()
            .clone();

        // Un-park the populated root (echo the exact string back).
        let resp = dispatch(
            Request::Unpark {
                path: populated_root.clone(),
            },
            &state,
        )
        .await;
        assert!(matches!(resp, Response::Ok), "got {resp:?}");

        // ListParked now shows only the empty root.
        match dispatch(Request::ListParked, &state).await {
            Response::Parked { paths } => {
                assert_eq!(paths.len(), 1);
                assert!(paths[0].ends_with("empty"));
            }
            other => panic!("expected Parked, got {other:?}"),
        }
        // And its parked site is gone from the listing.
        match dispatch(Request::ListSites, &state).await {
            Response::Sites { sites } => {
                assert!(
                    sites.iter().all(|s| s.name() != "blog"),
                    "blog should be gone after un-park: {sites:?}"
                );
            }
            other => panic!("expected Sites, got {other:?}"),
        }

        // Un-parking an absent path is an idempotent success.
        let resp = dispatch(
            Request::Unpark {
                path: populated_root,
            },
            &state,
        )
        .await;
        assert!(matches!(resp, Response::Ok), "absent un-park: got {resp:?}");
    }

    #[tokio::test]
    async fn set_secure_overrides_parked_keeping_kind_and_flips_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let sites_root = tmp.path().join("sites");
        std::fs::create_dir_all(sites_root.join("blog")).unwrap();
        let state = state_in(tmp.path());
        dispatch(Request::Park { path: sites_root }, &state).await;

        // Securing a parked site (mixed-case) records the override + sets flag.
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
                // Override applied, but the site stays parked (no promotion).
                assert_eq!(blog.kind(), yerd_core::SiteKind::Parked);
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
                http_port,
                https_port,
            } => {
                assert_eq!(dns_addr, state.dns_addr);
                assert_eq!(tld, "test");
                assert_eq!(ca_path, state.ca_path);
                // 64 lowercase hex chars; matches the stored fingerprint.
                assert_eq!(ca_fingerprint, state.ca_fingerprint.to_hex());
                assert_eq!(ca_fingerprint.len(), 64);
                assert_eq!(http_port, state.http.bound);
                assert_eq!(https_port, state.https.bound);
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

    #[cfg(unix)]
    #[tokio::test]
    async fn dispatch_restart_daemon_arms_flag_and_oks() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert!(!state
            .restart_requested
            .load(std::sync::atomic::Ordering::Acquire));
        let resp = dispatch(Request::RestartDaemon, &state).await;
        assert!(matches!(resp, Response::Ok), "got {resp:?}");
        // The flag is armed; `handle_client` trips shutdown after writing Ok.
        assert!(state
            .restart_requested
            .load(std::sync::atomic::Ordering::Acquire));
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

    #[tokio::test]
    async fn restart_php_not_installed_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match dispatch(
            Request::RestartPhp {
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
    async fn uninstall_php_not_installed_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match dispatch(
            Request::UninstallPhp {
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
    async fn uninstall_php_blocked_when_in_use_by_site() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, PhpVersion::new(8, 4));
        fake_install(&state.dirs, PhpVersion::new(8, 5));
        let app_dir = tmp.path().join("app");
        std::fs::create_dir_all(&app_dir).unwrap();
        dispatch(
            Request::Link {
                name: "app".into(),
                path: app_dir,
            },
            &state,
        )
        .await;
        dispatch(
            Request::SetPhp {
                name: "app".into(),
                version: PhpVersion::new(8, 5),
            },
            &state,
        )
        .await;

        match dispatch(
            Request::UninstallPhp {
                version: PhpVersion::new(8, 5),
            },
            &state,
        )
        .await
        {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::InvalidPath);
                assert!(message.contains("app"), "{message}");
            }
            other => panic!("expected InvalidPath (in use), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn uninstall_php_blocked_when_default_with_others() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, PhpVersion::new(8, 4));
        fake_install(&state.dirs, PhpVersion::new(8, 5));
        // Make 8.4 the default; 8.5 also installed; no sites.
        dispatch(
            Request::SetDefaultPhp {
                version: PhpVersion::new(8, 4),
            },
            &state,
        )
        .await;
        match dispatch(
            Request::UninstallPhp {
                version: PhpVersion::new(8, 4),
            },
            &state,
        )
        .await
        {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::InvalidPath);
                assert!(message.contains("default"), "{message}");
            }
            other => panic!("expected InvalidPath (is default), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn uninstall_php_succeeds_and_removes_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, PhpVersion::new(8, 4));
        fake_install(&state.dirs, PhpVersion::new(8, 5));
        dispatch(
            Request::SetDefaultPhp {
                version: PhpVersion::new(8, 4),
            },
            &state,
        )
        .await;

        let dir = state.dirs.data.join("php").join("php-8.5");
        assert!(dir.exists());
        match dispatch(
            Request::UninstallPhp {
                version: PhpVersion::new(8, 5),
            },
            &state,
        )
        .await
        {
            Response::PhpVersions { installed, .. } => {
                assert!(!installed.contains(&PhpVersion::new(8, 5)), "{installed:?}");
                assert!(installed.contains(&PhpVersion::new(8, 4)));
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }
        assert!(!dir.exists(), "version dir should be removed");
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

    #[tokio::test]
    async fn set_php_settings_persists_validates_and_resets() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        // Set two settings (no pools running → restart loop is a no-op).
        let resp = dispatch(
            Request::SetPhpSettings {
                settings: std::collections::BTreeMap::from([
                    ("memory_limit".to_string(), "512M".to_string()),
                    ("display_errors".to_string(), "on".to_string()),
                ]),
            },
            &state,
        )
        .await;
        match resp {
            Response::PhpVersions { settings, .. } => {
                assert_eq!(
                    settings.get("memory_limit").map(String::as_str),
                    Some("512M")
                );
                // Flag canonicalised to On.
                assert_eq!(
                    settings.get("display_errors").map(String::as_str),
                    Some("On")
                );
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }
        // Persisted to the live config.
        assert_eq!(
            state
                .config
                .lock()
                .await
                .php
                .settings
                .get("memory_limit")
                .map(String::as_str),
            Some("512M")
        );

        // Invalid value is rejected without mutating config.
        assert!(matches!(
            dispatch(
                Request::SetPhpSettings {
                    settings: std::collections::BTreeMap::from([(
                        "memory_limit".to_string(),
                        "bogus".to_string()
                    )]),
                },
                &state,
            )
            .await,
            Response::Error { .. }
        ));
        assert_eq!(
            state
                .config
                .lock()
                .await
                .php
                .settings
                .get("memory_limit")
                .map(String::as_str),
            Some("512M")
        );

        // Empty value removes (resets) the key.
        let resp = dispatch(
            Request::SetPhpSettings {
                settings: std::collections::BTreeMap::from([(
                    "memory_limit".to_string(),
                    String::new(),
                )]),
            },
            &state,
        )
        .await;
        match resp {
            Response::PhpVersions { settings, .. } => {
                assert!(!settings.contains_key("memory_limit"));
                assert!(settings.contains_key("display_errors"));
            }
            other => panic!("expected PhpVersions, got {other:?}"),
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
