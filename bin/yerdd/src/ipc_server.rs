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
                if !matches!(e, IpcError::UnexpectedEof { .. }) {
                    tracing::debug!(error = %e, "ipc decode error");
                }
                return;
            }
        };
        let resp = match req {
            Request::CreateSite { spec } => crate::create_site::start(spec, state.clone()).await,
            Request::InstallToolStreamed { tool } => {
                install_tool_streamed(tool, state.clone()).await
            }
            Request::InstallCloudflaredStreamed => {
                crate::tunnel::install_cloudflared_streamed(state.clone()).await
            }
            Request::CloudflaredLogin => crate::tunnel::named::login_streamed(state.clone()).await,
            Request::InstallPhpStreamed { version } => {
                install_php_streamed(version, state.clone()).await
            }
            Request::JobStatus { job_id, cursor } => state.jobs.poll(&job_id, cursor).await,
            Request::JobCancel { job_id } => state.jobs.cancel(&job_id).await,
            other => dispatch(other, &state).await,
        };
        if let Err(e) = write_message(&mut writer, &resp, DEFAULT_MAX_FRAME).await {
            tracing::debug!(error = %e, "ipc write error");
            return;
        }
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

#[allow(clippy::too_many_lines)]
async fn dispatch(req: Request, state: &DaemonState) -> Response {
    match req {
        Request::Ping => Response::Pong,
        Request::ListSites => {
            // `is_wordpress` is a cheap lookup into `state.wordpress_sites`,
            // refreshed on every router rebuild (a mutation or an
            // fs-watcher tick) rather than detected fresh here - this
            // handler is polled every few seconds and must not re-stat every
            // site's marker files on each poll. See `wordpress_detect`.
            let router = state.router.read().await;
            let tld = router.config().tld().to_owned();
            let wordpress_sites = state.wordpress_sites.read().await;
            let laravel_sites = state.laravel_sites.read().await;
            let entries = router
                .iter()
                .map(|site| {
                    let name = site.name();
                    let is_wordpress = wordpress_sites.get(name).copied().unwrap_or(false);
                    let uses_front_controller = site.uses_front_controller(is_wordpress);
                    let (primary_domain, domains) = site_entry_domains(&router, name, &tld);
                    let apex_shadowed_by = router.apex_shadowed_by(name).map(str::to_owned);
                    let is_laravel = laravel_sites.get(name).copied().unwrap_or(false);
                    yerd_ipc::SiteEntry {
                        site: site.clone(),
                        is_wordpress,
                        primary_domain,
                        domains,
                        apex_shadowed_by,
                        uses_front_controller,
                        is_laravel,
                    }
                })
                .collect();
            Response::Sites { sites: entries }
        }
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
        Request::DaemonInfo => {
            let cfg = state.config.lock().await;
            Response::Info {
                dns_addr: state.dns_addr,
                tld: cfg.tld.as_str().to_owned(),
                ca_path: state.ca_path.clone(),
                ca_fingerprint: state.ca_fingerprint.to_hex(),
                http_port: state.http.bound,
                https_port: state.https.bound,
                fallback_http: cfg.ports.fallback_http,
                fallback_https: cfg.ports.fallback_https,
                dns_port: cfg.dns_port,
            }
        }
        Request::Park { .. }
        | Request::Link { .. }
        | Request::Unlink { .. }
        | Request::Unpark { .. }
        | Request::SetPhp { .. }
        | Request::SetSecure { .. }
        | Request::SetWebRoot { .. }
        | Request::SetWordpressAutoLogin { .. }
        | Request::SetFrontController { .. }
        | Request::AddDomain { .. }
        | Request::RemoveDomain { .. }
        | Request::SetPrimaryDomain { .. }
        | Request::ResetDomains { .. }
        | Request::RemoveProxy { .. }
        | Request::RemoveProxyRule { .. } => handle_mutation(req, state).await,
        Request::AddProxy { ref url, .. } | Request::AddProxyRule { ref url, .. } => {
            if is_self_forward(url, &[state.http.bound, state.https.bound]) {
                Response::Error {
                    code: ErrorCode::InvalidPath,
                    message: "proxy target points at yerd's own listening port (routing loop)"
                        .to_owned(),
                }
            } else {
                handle_mutation(req, state).await
            }
        }
        Request::ListProxies => list_proxies(state).await,
        Request::ListGroups => {
            let cfg = state.config.lock().await;
            Response::Groups {
                order: cfg.groups.order.clone(),
                members: cfg.groups.members.clone(),
            }
        }
        Request::CreateGroup { .. }
        | Request::DeleteGroup { .. }
        | Request::SetGroupOrder { .. }
        | Request::SetSiteGroup { .. }
        | Request::RenameGroup { .. } => handle_group_mutation(req, state).await,
        Request::ListPhp => php_versions_response(state).await,
        Request::InstallPhp { version } => install_php(version, state).await,
        Request::SetDefaultPhp { version } => set_default_php(version, state).await,
        Request::CheckPhpUpdates => {
            let dl = crate::php_install::ReqwestDownloader::new();
            crate::php_updates::poll_and_refresh(state, &dl, yerd_update::PHP_LISTING_PUBLIC_KEY)
                .await;
            php_versions_response(state).await
        }
        Request::UpdatePhp { version } => update_php(version, state).await,
        Request::AvailablePhp => available_php_response(state).await,
        Request::SetPhpSettings { settings } => set_php_settings(settings, state).await,
        Request::AddPhpExtension {
            version,
            path,
            name,
            zend,
        } => add_php_extension(version, path, name, zend, state).await,
        Request::RemovePhpExtension { version, name } => {
            remove_php_extension(version, name, state).await
        }
        Request::ListPhpExtensions => list_php_extensions(state).await,
        Request::RestartPhp { version } => restart_php(version, state).await,
        Request::RestartAllPhp => restart_all_php(state).await,
        Request::UninstallPhp { version } => uninstall_php(version, state).await,
        Request::Status => Response::Status {
            report: Box::new(build_status_report(state).await),
        },
        Request::Diagnose => Response::Diagnoses {
            items: yerd_doctor::diagnose(
                &build_status_report(state).await,
                path_needs_setup(state),
            ),
        },
        Request::DoctorFix => run_doctor_fix(state).await,
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
        Request::AvailableWordpressVersions => {
            let dl = crate::php_install::ReqwestDownloader::new();
            crate::wordpress_versions::available_versions(state, &dl).await
        }
        Request::MintWordpressLoginToken { site } => {
            crate::wordpress_login::mint_wordpress_login_token(&site, state).await
        }
        Request::WordpressAdminUsers { site } => {
            crate::wordpress_users::admin_users(&site, state).await
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
        Request::AddService {
            type_id,
            site,
            port,
            version,
            autostart,
        } => {
            let dl = crate::php_install::ReqwestDownloader::new();
            crate::services::add_service(
                &type_id,
                site.as_deref(),
                port,
                version.as_deref(),
                autostart,
                state,
                &dl,
            )
            .await
        }
        Request::RemoveService { service, purge } => {
            crate::services::remove_service(&service, purge, state).await
        }
        Request::SetServiceAutostart { service, enabled } => {
            crate::services::set_service_autostart(&service, enabled, state).await
        }
        Request::SetServiceSite { service, site } => {
            crate::services::set_service_site(&service, &site, state).await
        }
        Request::AddableServiceTypes => {
            let dl = crate::php_install::ReqwestDownloader::new();
            crate::services::addable_service_types(state, &dl).await
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
        Request::MarkMailsRead { ids } => match state.mail_store.mark_read(&ids).await {
            Ok(()) => Response::Ok,
            Err(e) => internal(format!("mail mark-read failed: {e}")),
        },
        Request::SetMailPort { port } => set_mail_port(port, state).await,
        Request::SetFallbackPorts { http, https } => set_fallback_ports(http, https, state).await,
        Request::SetDnsPort { port } => set_dns_port(port, state).await,
        Request::SetMailEnabled { enabled } => set_mail_enabled(enabled, state).await,
        Request::SetSymlinkProtection { enabled } => set_symlink_protection(enabled, state).await,
        Request::ListTools => Response::Tools {
            tools: list_tools_with_external(state).await,
        },
        Request::InstallTool { tool } => install_tool(&tool, state).await,
        Request::UninstallTool { tool } => uninstall_tool(&tool, state).await,
        Request::CheckUpdate { channel } => {
            let dl = crate::php_install::ReqwestDownloader::new();
            crate::self_update::check_update(channel, state, &dl).await
        }
        Request::CachedUpdateStatus => crate::self_update::cached_update_status(state).await,
        Request::SetUpdateChannel { channel } => {
            crate::self_update::set_update_channel(channel, state).await
        }
        Request::StageUpdate { channel } => {
            let dl = crate::php_install::ReqwestDownloader::new();
            crate::self_update::stage_update(channel, state, &dl, yerd_update::UPDATE_PUBLIC_KEY)
                .await
        }
        Request::StartQuickTunnel { site } => crate::tunnel::start_quick_tunnel(&site, state).await,
        Request::StopTunnel { site } => crate::tunnel::stop_tunnel(&site, state).await,
        Request::TunnelStatus => crate::tunnel::tunnel_status(state).await,
        Request::CreateNamedTunnel { name } => crate::tunnel::named::create(&name, state).await,
        Request::ListNamedTunnels => crate::tunnel::named::list(state).await,
        Request::RouteTunnelDns { tunnel, hostname } => {
            crate::tunnel::named::route_dns(&tunnel, &hostname, state).await
        }
        Request::SetSiteTunnel { site, hostname } => {
            crate::tunnel::named::set_site_hostname(&site, hostname.as_deref(), state).await
        }
        Request::StartNamedTunnel => crate::tunnel::named::start(state).await,
        Request::StopNamedTunnel => crate::tunnel::named::stop(state).await,
        Request::DeleteNamedTunnel { name } => crate::tunnel::named::delete(&name, state).await,
        _ => Response::Error {
            code: ErrorCode::Internal,
            message: "unsupported request".into(),
        },
    }
}

/// Compute a site's `SiteEntry` domain fields. Returns `(primary_domain,
/// domains)`, both **omitted** (`None`/empty) for an effectively-default site
/// (apex only, primary = apex) so the wire shape stays byte-identical to older
/// clients. For a customized site, `domains` is the full effective set as FQDNs
/// in router order (apex-first-then-added, so a non-apex primary is *not*
/// necessarily first) and `primary_domain` is set only when the primary differs
/// from the default apex. Clients identify the primary by matching
/// `primary_domain`, not by position.
fn site_entry_domains(
    router: &yerd_core::SiteRouter,
    name: &str,
    tld: &str,
) -> (Option<String>, Vec<String>) {
    let apex = yerd_core::Domain::apex(name);
    let effective = router.effective_domains(name).unwrap_or(&[]);
    let primary = router.primary_domain(name);

    let is_default =
        effective.len() == 1 && effective.first() == Some(&apex) && primary == Some(&apex);
    if is_default {
        return (None, Vec::new());
    }

    let domains = effective.iter().map(|d| d.to_fqdn(tld)).collect();
    let primary_domain = match primary {
        Some(p) if *p != apex => Some(p.to_fqdn(tld)),
        _ => None,
    };
    (primary_domain, domains)
}

/// Installed PHP versions (the bundled installs in yerd's data dir), ascending
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

/// `available php` - list the major.minor versions installable from the signed
/// `php.json` manifest, plus what's already installed (so clients hide or tag
/// them). Fetches + verifies the manifest on demand; a fetch/transport OR
/// signature-verification failure is an error (an empty parse result is still a
/// valid empty list).
async fn available_php_response(state: &DaemonState) -> Response {
    let dl = crate::php_install::ReqwestDownloader::new();
    available_php_with(state, &dl, yerd_update::PHP_LISTING_PUBLIC_KEY).await
}

/// Injectable core of [`available_php_response`] (the downloader is a parameter
/// so tests can feed a fixture listing without touching the network).
async fn available_php_with(
    state: &DaemonState,
    dl: &dyn yerd_php::Downloader,
    public_key: &str,
) -> Response {
    let (os, arch) = match yerd_php::current_os_arch() {
        Ok(p) => p,
        Err(e) => {
            return Response::Error {
                code: php_error_code(&e),
                message: e.to_string(),
            }
        }
    };
    let listing = match crate::php_install::fetch_verified_listing(dl, public_key).await {
        Ok(body) => body,
        Err(e) => return internal(format!("couldn't load the PHP listing: {e}")),
    };
    Response::AvailablePhp {
        available: yerd_php::available_minors(&listing, os, arch),
        installed: installed_versions(state),
    }
}

/// Whether a dev tool is installed but Yerd's `{data}/bin` isn't on the user's
/// PATH yet (no managed block in any known shell rc) - drives the doctor's
/// [`yerd_ipc::DiagnosisCode::BinDirNotOnPath`] warning. `Some(false)` when no
/// tool is installed or PATH is already wired; `None` when undeterminable
/// (non-Unix, or `$HOME` unset). Computed on demand from the `Diagnose` handler,
/// not on the per-poll status path. The cover/pcov shims alone don't count - the
/// gate is an actual installed dev tool.
fn path_needs_setup(state: &DaemonState) -> Option<bool> {
    #[cfg(not(unix))]
    {
        let _ = state;
        None
    }
    #[cfg(unix)]
    {
        use yerd_platform::pure::shell_profile::{self, rc_relpaths, HostOs, Shell};

        let any_tool = crate::tools::list_status(&state.dirs)
            .iter()
            .any(|t| t.installed);
        if !any_tool {
            return Some(false);
        }
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(std::path::PathBuf::from)?;
        let os = if cfg!(target_os = "macos") {
            HostOs::MacOs
        } else {
            HostOs::Linux
        };
        let present = [Shell::Zsh, Shell::Bash, Shell::Fish, Shell::Posix]
            .into_iter()
            .flat_map(|s| rc_relpaths(s, os))
            .any(|rel| {
                std::fs::read_to_string(home.join(rel))
                    .is_ok_and(|c| shell_profile::contains_block(&c))
            });
        Some(!present)
    }
}

/// Assemble a read-only [`yerd_ipc::StatusReport`].
///
/// Lock discipline: each guard is acquired, drained into owned data, and dropped
/// before the next acquisition - never two at once, never a guard held across an
/// `.await` that touches another lock. Mirrors the hazard documented in
/// `handle_mutation`.
/// Resident-set size for each of `pids`, gathered in a single `spawn_blocking`.
///
/// `SystemMetrics::rss_bytes` shells out to `ps` on macOS (fork+exec+wait) -
/// genuinely blocking I/O, unlike every other field of a `StatusReport`. Doing
/// it once off-executor, rather than synchronously per pid inline, keeps a
/// tokio worker thread from being tied up once per installed PHP version plus
/// once for the daemon itself on every `Request::Status`/`Request::Diagnose`
/// (the GUI polls this every ~6s), which under load could starve the whole
/// worker pool. Missing pids are simply absent from the returned map.
async fn collect_rss_by_pid(
    metrics: yerd_platform::ActiveSystemMetrics,
    pids: Vec<u32>,
) -> std::collections::HashMap<u32, u64> {
    use yerd_platform::SystemMetrics;
    tokio::task::spawn_blocking(move || {
        let mut out = std::collections::HashMap::new();
        for pid in pids {
            if let Some(rss) = metrics.rss_bytes(pid) {
                out.insert(pid, rss);
            }
        }
        out
    })
    .await
    .unwrap_or_default()
}

/// Convert domain collisions (live sites plus persisted `[domains]` deltas) into
/// per-losing-site shadow records for the status report, de-duplicated on
/// `(site, winner)` so a site that loses several domains to one winner appears
/// once. The common entry is a shadowed apex; a hand-edited config can also
/// collide two sites on an explicit domain.
fn domain_shadows(
    cfg: &yerd_config::Config,
    sites: Vec<yerd_core::Site>,
) -> Vec<yerd_ipc::DomainShadow> {
    let mut out: Vec<yerd_ipc::DomainShadow> = Vec::new();
    for collision in crate::site_domains::collisions(cfg, sites) {
        for loser in collision.losers {
            let entry = yerd_ipc::DomainShadow {
                site: loser,
                shadowed_by: collision.winner.clone(),
            };
            if !out.contains(&entry) {
                out.push(entry);
            }
        }
    }
    out
}

/// Builds the full [`StatusReport`]. The config lock is held across the router
/// snapshot (config-then-router, the same order `handle_mutation` takes) so
/// `domain_shadows` sees a consistent (config, router) pair rather than one from
/// either side of a concurrent mutation.
#[allow(clippy::too_many_lines)]
async fn build_status_report(state: &DaemonState) -> yerd_ipc::StatusReport {
    use yerd_platform::SystemMetrics;

    let (sites, tld, default_php, mail_enabled, mail_port, symlink_protection, shadows) = {
        let cfg = state.config.lock().await;
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
        let site_snapshot: Vec<yerd_core::Site> = router.iter().cloned().collect();
        let shadows = domain_shadows(&cfg, site_snapshot);
        (
            counts,
            cfg.tld.as_str().to_owned(),
            cfg.php.default,
            cfg.mail.enabled,
            cfg.mail.port,
            cfg.symlink_protection,
            shadows,
        )
    };

    let snapshots = {
        let mut mgr = state.php_manager.lock().await;
        mgr.snapshots()
    };

    let installed = installed_versions(state);
    let updates = crate::php_updates::cached_updates(state).await;

    let metrics = yerd_platform::ActiveSystemMetrics::new();

    let daemon_pid = std::process::id();
    let pids: Vec<u32> = installed
        .iter()
        .filter_map(|v| {
            snapshots
                .iter()
                .find(|s| s.version == *v)
                .and_then(|s| s.pid)
        })
        .chain(std::iter::once(daemon_pid))
        .collect();
    let rss_by_pid = collect_rss_by_pid(metrics, pids).await;

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
                rss_bytes: pid.and_then(|p| rss_by_pid.get(&p).copied()),
                update_available: updates
                    .iter()
                    .find(|u| u.version == *v)
                    .map(|u| u.latest.clone()),
            }
        })
        .collect();

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

    let (port_redirect, foreign_web_listener) = tokio::task::spawn_blocking(|| {
        use yerd_platform::PortRedirector;
        let r = yerd_platform::ActivePortRedirector::new();
        (r.is_active(), r.foreign_web_listener())
    })
    .await
    .unwrap_or((None, None));

    let backup_tld = tld.clone();
    let resolver_backup = tokio::task::spawn_blocking(move || latest_resolver_backup(&backup_tld))
        .await
        .ok()
        .flatten();

    let load_avg = metrics
        .load_average()
        .map(|[a, b, c]| [load_to_centi(a), load_to_centi(b), load_to_centi(c)]);

    let (mail_count, mail_unread) = state.mail_store.counts().await;
    let shared_sites = crate::tunnel::shared_site_count(state).await;

    yerd_ipc::StatusReport {
        daemon_pid: std::process::id(),
        uptime_secs: state.started_at.elapsed().as_secs(),
        daemon_rss_bytes: rss_by_pid.get(&daemon_pid).copied(),
        tld,
        http: state.http,
        https: state.https,
        dns_addr: state.dns_addr,
        ca: yerd_ipc::CaStatus {
            path: state.ca_path.clone(),
            fingerprint: state.ca_fingerprint.to_hex(),
            trusted_system,
            php_trusts_ca: php_trusts_ca(state).await,
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
            count: mail_count,
            unread: mail_unread,
        }),
        web_unbound: state.web_unbound,
        dns_unbound: state.dns_unbound,
        boot_id: Some(state.boot_id),
        shared_sites,
        symlink_protection,
        shadows,
    }
}

/// The path of the most recent replaced-resolver backup for `tld`, if one was
/// saved within the last 7 days. macOS-only - the helper writes these when it
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

/// `doctor fix` - run the safe auto-fixes, then re-diagnose for the remainder.
async fn run_doctor_fix(state: &DaemonState) -> Response {
    let report = build_status_report(state).await;
    let mut performed: Vec<yerd_ipc::FixResult> = Vec::new();

    for action in yerd_doctor::plan_auto_fixes(&report) {
        match action {
            yerd_doctor::FixAction::RestartFpm(v) => {
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
            yerd_doctor::FixAction::RebuildPhpCaBundle => {
                let rebuilt = rebuild_php_ca_bundle(state).await;
                performed.push(yerd_ipc::FixResult {
                    code: yerd_ipc::DiagnosisCode::PhpCaNotTrusted,
                    ok: rebuilt,
                    message: if rebuilt {
                        "rebuilt the PHP CA bundle".to_owned()
                    } else {
                        "could not rebuild the PHP CA bundle; see the daemon logs for details"
                            .to_owned()
                    },
                });
            }
            other => {
                tracing::warn!(?other, "unhandled doctor auto-fix action");
            }
        }
    }

    let after = build_status_report(state).await;
    let manual = yerd_doctor::diagnose(&after, path_needs_setup(state))
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

/// `update php [<ver>]` - upgrade the given minor (or all installed) to the
/// latest published build when newer; restart the updated pools; refresh the
/// cache; return the new list.
///
/// Update-all skips only a minor that is genuinely absent from the manifest
/// (`VersionUnavailable`); a manifest-wide fault (parse/schema/untrusted) and any
/// failure for a **targeted** update (`version: Some`) are surfaced as an error
/// rather than a silent no-op. If a later minor's install fails, the minors that
/// already updated are still finalised (pools restarted, cache refreshed) before
/// the error is returned.
///
/// Holds `php_mutate` across the install loop so it can't race
/// `install_php`/`install_php_streamed` over the same per-version staging dir.
/// Not re-entrant: dispatched directly, never while `php_mutate` is already held.
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
                    message: format!("PHP {v} is not installed - run `yerd install php {v}`"),
                };
            }
            vec![v]
        }
        None => crate::php_updates::installed_minors(state),
    };
    let listing =
        match crate::php_install::fetch_verified_listing(&dl, yerd_update::PHP_LISTING_PUBLIC_KEY)
            .await
        {
            Ok(body) => body,
            Err(e) => return internal(format!("listing fetch/verify failed: {e}")),
        };
    let _guard = state.php_mutate.lock().await;
    let mut updated: Vec<yerd_core::PhpVersion> = Vec::new();
    let mut pending_error: Option<yerd_php::PhpError> = None;
    for minor in targets {
        let Some(installed) = crate::php_install::installed_patch(&state.dirs, minor) else {
            continue;
        };
        let installed_rev = crate::php_install::installed_revision(&state.dirs, minor);
        let artifact = match yerd_php::resolve_from_listing(&listing, minor, os, arch) {
            Ok(a) => a,
            Err(yerd_php::PhpError::VersionUnavailable { .. }) if version.is_none() => continue,
            Err(e) => {
                pending_error = Some(e);
                break;
            }
        };
        if yerd_php::is_newer_build(
            &installed,
            installed_rev,
            &artifact.full_version,
            artifact.revision,
        ) {
            if let Err(e) = crate::php_install::install(
                minor,
                &state.dirs,
                &dl,
                yerd_update::PHP_LISTING_PUBLIC_KEY,
                None,
            )
            .await
            {
                tracing::error!(version = %minor, error = %e, "PHP update failed");
                pending_error = Some(e);
                break;
            }
            tracing::info!(version = %minor, from = %installed, to = %artifact.full_version, "updated PHP");
            updated.push(minor);
        }
    }
    restart_updated_pools(state, &updated).await;
    crate::php_updates::poll_and_refresh(state, &dl, yerd_update::PHP_LISTING_PUBLIC_KEY).await;
    if let Some(e) = pending_error {
        return Response::Error {
            code: php_error_code(&e),
            message: e.to_string(),
        };
    }
    php_versions_response(state).await
}

/// Restart the FPM pool of each just-updated minor that has a **started** pool
/// (running or crashed/`Failed`), so it re-execs the freshly-installed binary
/// instead of the stale process. A never-started / stopped ondemand pool is left
/// alone (the next request spawns it from the new binary), matching
/// [`restart_all_php`]'s semantics. Per-pool failures are logged, not fatal - the
/// update itself already succeeded. Runs under the caller's `php_mutate` guard.
async fn restart_updated_pools(state: &DaemonState, updated: &[yerd_core::PhpVersion]) {
    if updated.is_empty() {
        return;
    }
    let mut mgr = state.php_manager.lock().await;
    let active: std::collections::HashSet<yerd_core::PhpVersion> =
        mgr.snapshots().into_iter().map(|s| s.version).collect();
    for minor in pools_needing_restart(&active, updated) {
        match mgr.restart(minor).await {
            Ok(_) => tracing::info!(version = %minor, "restarted FPM pool after PHP update"),
            Err(e) => {
                tracing::warn!(version = %minor, error = %e, "failed to restart FPM pool after PHP update");
            }
        }
    }
}

/// The just-updated minors whose pool is currently active, preserving `updated`
/// order. Updated-but-inactive minors are excluded so an update never *starts* a
/// pool the user had stopped - it only re-execs one already running.
fn pools_needing_restart(
    active: &std::collections::HashSet<yerd_core::PhpVersion>,
    updated: &[yerd_core::PhpVersion],
) -> Vec<yerd_core::PhpVersion> {
    updated
        .iter()
        .copied()
        .filter(|m| active.contains(m))
        .collect()
}

/// `install php <ver>` - download + verify + unpack a prebuilt build. Runs the
/// (slow) download with no config lock held; the per-connection task model means
/// other clients are unaffected. Synchronous (the CLI's `yerd php install` path);
/// the GUI uses [`install_php_streamed`] for live progress.
///
/// Serializes installs under `php_mutate` (the staging dir is keyed by
/// version + pid, so concurrent installs of the same version would clobber each
/// other). A failure is logged as the only durable record of it (the line the
/// GUI diagnostics / About > Logs panel tails).
async fn install_php(version: yerd_core::PhpVersion, state: &DaemonState) -> Response {
    let dl = crate::php_install::ReqwestDownloader::new();
    let _guard = state.php_mutate.lock().await;
    match crate::php_install::install(
        version,
        &state.dirs,
        &dl,
        yerd_update::PHP_LISTING_PUBLIC_KEY,
        None,
    )
    .await
    {
        Ok(()) => {
            finalize_php_install(version, state).await;
            Response::Ok
        }
        Err(e) => {
            tracing::error!(%version, error = %e, "PHP install failed");
            Response::Error {
                code: php_error_code(&e),
                message: e.to_string(),
            }
        }
    }
}

/// Post-install bookkeeping shared by the sync and streamed install paths: teach
/// the live `PhpManager` about the new binaries (its binary map is a startup
/// snapshot, so this lets the proxy spawn the new FPM pool without a daemon
/// restart), adopt the first install as the default so the `php` shim exists and
/// sites have a runtime, then bundle pcov + rebuild shims. Adopting the default
/// runs before the shim rebuild so the shims are built against the new default.
/// All best-effort - the install itself has already succeeded by the time this
/// runs.
async fn finalize_php_install(version: yerd_core::PhpVersion, state: &DaemonState) {
    refresh_php_binaries(state).await;
    adopt_default_if_unset(version, state).await;
    refresh_pcov_and_shims(state).await;
}

/// `InstallPhpStreamed` - download + unpack a PHP build as a background job,
/// streaming phase + byte-count progress into the job log. Returns `JobStarted`
/// immediately; the client polls `JobStatus`. The streaming sibling of
/// [`install_php`] (used by the GUI onboarding + PHP screen) so a multi-minute
/// download shows progress and can be cancelled instead of spinning a request.
///
/// The `php_mutate` lock is acquired by racing it against `JobCancel` so a job
/// queued behind another install can cancel without waiting for the lock, and is
/// held through [`finalize_php_install`] so no other PHP mutation interleaves
/// with the default/shim/manager updates. On cancel the install future is
/// dropped: its only side effects are in a `.staging-` dir the next install
/// clears, so an interrupted download leaves nothing half-installed. Every arm
/// closes the progress channel and drains it before finishing the job, so no log
/// line is lost.
pub(crate) async fn install_php_streamed(
    version: yerd_core::PhpVersion,
    state: Arc<DaemonState>,
) -> Response {
    let (job_id, mut cancel) = state.jobs.create().await;
    let id = job_id.clone();
    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let drain = {
            let state = state.clone();
            let id = id.clone();
            tokio::spawn(async move {
                while let Some(line) = rx.recv().await {
                    state.jobs.push_log(&id, line).await;
                }
            })
        };

        state
            .jobs
            .set_phase(&id, format!("Installing PHP {version}"))
            .await;
        let dl = crate::php_install::ReqwestDownloader::new();
        let guard = tokio::select! {
            g = state.php_mutate.lock() => g,
            _ = cancel.changed() => {
                drop(tx);
                let _ = drain.await;
                state
                    .jobs
                    .finish(&id, yerd_ipc::JobState::Cancelled, None)
                    .await;
                return;
            }
        };
        let result = tokio::select! {
            r = crate::php_install::install(version, &state.dirs, &dl, yerd_update::PHP_LISTING_PUBLIC_KEY, Some(&tx)) => Some(r),
            _ = cancel.changed() => None,
        };

        match result {
            Some(Ok(())) => {
                finalize_php_install(version, &state).await;
                drop(guard);
                drop(tx);
                let _ = drain.await;
                state
                    .jobs
                    .finish(&id, yerd_ipc::JobState::Succeeded, None)
                    .await;
            }
            Some(Err(e)) => {
                drop(guard);
                drop(tx);
                let _ = drain.await;
                tracing::error!(%version, error = %e, "PHP install failed");
                state
                    .jobs
                    .finish(&id, yerd_ipc::JobState::Failed, Some(e.to_string()))
                    .await;
            }
            None => {
                drop(guard);
                drop(tx);
                let _ = drain.await;
                state
                    .jobs
                    .finish(&id, yerd_ipc::JobState::Cancelled, None)
                    .await;
            }
        }
    });
    Response::JobStarted { job_id }
}

/// On the *first* successful install - when the configured default PHP isn't
/// actually installed yet - adopt the just-installed `version` as the default,
/// so the `php` shim gets created and sites have a runtime. No-op once a real
/// default is installed (later installs never steal the default).
///
/// Lock-safe: the "is the current default installed?" check and the set happen
/// under the config lock, so two concurrent first-installs can't both win.
/// Best-effort (the install already succeeded) and does NOT reconcile shims -
/// the caller's `refresh_pcov_and_shims` reconciles against the updated default.
async fn adopt_default_if_unset(version: yerd_core::PhpVersion, state: &DaemonState) {
    let mut cfg_guard = state.config.lock().await;
    if crate::php_install::cli_binary_path(&state.dirs, cfg_guard.php.default).exists() {
        return;
    }
    let mut new = cfg_guard.clone();
    new.php.default = version;
    if let Some(yerd_bin) = yerd_sibling() {
        if let Err(e) = crate::php_install::set_default_shim(&state.dirs, &yerd_bin) {
            tracing::warn!(error = %e, "auto-default shim update failed");
        }
    } else {
        tracing::warn!("cannot locate the `yerd` binary; skipping php shim update");
    }
    if let Err(e) = new.save(&state.config_path) {
        tracing::warn!(error = %e, "auto-default config save failed");
        return;
    }
    *cfg_guard = new;
    tracing::info!(version = %version, "adopted first installed PHP as the default");
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

/// Absolute path to the `yerd` CLI binary, assumed a sibling of the running
/// `yerdd` (mirrors `yerd`'s own `elevate::sibling_binaries`). This is the target
/// the cover shims (`phpcover`/`php<ver>cover`) symlink to.
fn yerd_sibling() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("yerd"))
}

/// Reconcile the managed PHP shims (`php`/`php<ver>`/`phpcover`/`php<ver>cover`),
/// serialized behind the shim mutex. Best-effort: failures are logged. Each shim
/// is a wrapper that resolves its version (and the default from config) at run
/// time, so this needs no default argument.
async fn reconcile_shims_for(state: &DaemonState) {
    let Some(yerd_bin) = yerd_sibling() else {
        tracing::warn!("cannot locate the `yerd` binary; skipping PHP-shim reconcile");
        return;
    };
    let _guard = state.shim_reconcile.lock().await;
    if let Err(e) = crate::php_install::reconcile_shims(&state.dirs, &yerd_bin) {
        tracing::warn!(error = %e, "PHP-shim reconcile failed");
    }
}

/// Rebuild `{data}/cacert.pem` (host roots + Yerd CA) from the current host
/// trust store. Returns whether the file now contains public roots. Because FPM
/// and the CLI read the bundle by its stable path at TLS-handshake / invocation
/// time, restoring a deleted/stale file takes effect without a pool restart.
async fn rebuild_php_ca_bundle(state: &DaemonState) -> bool {
    let dirs = state.dirs.clone();
    let ca_path = state.ca_path.clone();
    tokio::task::spawn_blocking(move || {
        use yerd_platform::TrustStore;
        let Ok(ca_pem) = std::fs::read_to_string(&ca_path) else {
            return false;
        };
        let roots = yerd_platform::ActiveTrustStore
            .system_root_bundle()
            .ok()
            .flatten();
        crate::startup::build_php_ca_bundle(&dirs, &ca_pem, roots.as_deref()).is_some()
    })
    .await
    .unwrap_or(false)
}

/// Probe whether the bundled PHP trusts the Yerd CA: `None` when the feature is
/// off (no managed bundle wired at startup), else `Some(true)` when
/// `{data}/cacert.pem` exists and contains the CA cert, `Some(false)` otherwise
/// (missing / stale bundle → PHP HTTPS to `.test` fails).
async fn php_trusts_ca(state: &DaemonState) -> Option<bool> {
    let bundle = state.php_ca_bundle.clone()?;
    let ca_path = state.ca_path.clone();
    tokio::task::spawn_blocking(move || {
        match (
            std::fs::read_to_string(&ca_path),
            std::fs::read_to_string(&bundle),
        ) {
            (Ok(ca), Ok(b)) => bundle_contains_ca(&ca, &b),
            _ => false,
        }
    })
    .await
    .ok()
}

/// Whether `bundle_pem` embeds the CA certificate `ca_pem` (both read from disk,
/// both originating from the same `cert_pem()` so the CA block is byte-identical
/// and a contiguous substring). An empty/whitespace-only `ca_pem` (corrupt CA
/// file) is never treated as trusted, to avoid the trivial `contains("")` match.
fn bundle_contains_ca(ca_pem: &str, bundle_pem: &str) -> bool {
    let ca = ca_pem.trim();
    !ca.is_empty() && bundle_pem.contains(ca)
}

pub(crate) async fn write_cli_ini_now(state: &DaemonState) {
    let (settings, extensions) = {
        let cfg = state.config.lock().await;
        (cfg.php.settings.clone(), cfg.php.extensions.clone())
    };
    if let Err(e) = crate::php_install::write_cli_ini(
        &state.dirs,
        &settings,
        state.php_ca_bundle.as_deref(),
        &extensions,
    ) {
        tracing::warn!(error = %e, "failed to write CLI php.ini");
    }
}

/// Reconcile the managed PHP shims against the installed set.
async fn reconcile_shims_now(state: &DaemonState) {
    reconcile_shims_for(state).await;
}

/// Reconcile the dev-tool shims (`composer`/`node`/`npm`/`npx`/`bun`/`bunx`) under
/// the **shared** `shim_reconcile` mutex (same dir as the PHP reconcile).
/// Best-effort: failures are logged. Used at startup and after install/uninstall.
pub(crate) async fn reconcile_tool_shims_now(state: &DaemonState) {
    let Some(yerd_bin) = yerd_sibling() else {
        tracing::warn!("cannot locate the `yerd` binary; skipping tool-shim reconcile");
        return;
    };
    let _guard = state.shim_reconcile.lock().await;
    if let Err(e) = crate::tools::reconcile_tool_shims(&state.dirs, &yerd_bin) {
        tracing::warn!(error = %e, "tool-shim reconcile failed");
    }
}

/// Build the tool list and tag any *not* Yerd-managed tool that's available on
/// the user's PATH as `external` (Tooling shows "External", no actions). Skips the
/// (login-shell) PATH resolution entirely when everything is already managed.
async fn list_tools_with_external(state: &DaemonState) -> Vec<yerd_ipc::ToolStatus> {
    let mut tools = crate::tools::list_status(&state.dirs);
    if tools.iter().all(|t| t.installed) {
        return tools;
    }
    let Some(dirs) = crate::tools::external::resolve_user_path().await else {
        return tools;
    };
    let data_bin = crate::tools::bin_dir(&state.dirs);
    let data_root = &state.dirs.data;
    for t in &mut tools {
        if t.installed {
            continue;
        }
        if let Some(tool) = crate::tools::Tool::parse(&t.id) {
            let found = crate::tools::external::external_tool(&dirs, tool, &data_bin, data_root);
            t.external = found.is_some();
            t.external_path = found.map(|p| p.display().to_string());
        }
    }
    tools
}

/// `install tool <id>` - download + verify the latest release, then (re)build its
/// `{data}/bin` shims. Runs the slow download with no lock held.
async fn install_tool(tool: &str, state: &DaemonState) -> Response {
    let Some(t) = crate::tools::Tool::parse(tool) else {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("unknown tool {tool:?}"),
        };
    };
    let dl = crate::php_install::ReqwestDownloader::new();
    let _mutate = state.tool_mutate.lock().await;
    match crate::tools::install(t, &state.dirs, &dl, None).await {
        Ok(()) => {
            reconcile_tool_shims_now(state).await;
            Response::Ok
        }
        Err(e) => Response::Error {
            code: tool_error_code(&e),
            message: e.to_string(),
        },
    }
}

/// `InstallToolStreamed` - install a tool as a background job, streaming its
/// output (Composer's, for the Laravel installer) into the job log. Returns
/// `JobStarted` immediately; the client polls `JobStatus`.
pub(crate) async fn install_tool_streamed(tool: String, state: Arc<DaemonState>) -> Response {
    let Some(t) = crate::tools::Tool::parse(&tool) else {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("unknown tool {tool:?}"),
        };
    };
    let (job_id, mut cancel) = state.jobs.create().await;
    let id = job_id.clone();
    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let drain = {
            let state = state.clone();
            let id = id.clone();
            tokio::spawn(async move {
                while let Some(line) = rx.recv().await {
                    state.jobs.push_log(&id, line).await;
                }
            })
        };

        state
            .jobs
            .set_phase(&id, format!("Installing {}", t.display_name()))
            .await;
        let dl = crate::php_install::ReqwestDownloader::new();
        let guard = state.tool_mutate.lock().await;
        let result = tokio::select! {
            r = crate::tools::install(t, &state.dirs, &dl, Some(&tx)) => Some(r),
            _ = cancel.changed() => None,
        };
        drop(guard);
        drop(tx);
        let _ = drain.await;

        match result {
            Some(Ok(())) => {
                reconcile_tool_shims_now(&state).await;
                state
                    .jobs
                    .finish(&id, yerd_ipc::JobState::Succeeded, None)
                    .await;
            }
            Some(Err(e)) => {
                state
                    .jobs
                    .finish(&id, yerd_ipc::JobState::Failed, Some(e.to_string()))
                    .await;
            }
            None => {
                state
                    .jobs
                    .finish(&id, yerd_ipc::JobState::Cancelled, None)
                    .await;
            }
        }
    });
    Response::JobStarted { job_id }
}

/// `uninstall tool <id>` - remove the tool's files, then prune its shims.
async fn uninstall_tool(tool: &str, state: &DaemonState) -> Response {
    let Some(t) = crate::tools::Tool::parse(tool) else {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("unknown tool {tool:?}"),
        };
    };
    let _mutate = state.tool_mutate.lock().await;
    match crate::tools::uninstall(&state.dirs, t) {
        Ok(()) => {
            reconcile_tool_shims_now(state).await;
            Response::Ok
        }
        Err(e) => Response::Error {
            code: tool_error_code(&e),
            message: e.to_string(),
        },
    }
}

/// Map a [`crate::tools::ToolError`] to an IPC error code (mirrors `php_error_code`).
fn tool_error_code(e: &crate::tools::ToolError) -> ErrorCode {
    use crate::tools::ToolError;
    match e {
        ToolError::Unknown(_) => ErrorCode::NotFound,
        ToolError::UnsupportedHost(_) => ErrorCode::InvalidPath,
        _ => ErrorCode::Internal,
    }
}

/// Best-effort: fetch the `pcov` `.so` for installed PHP versions, then rebuild
/// the cover/clean versioned CLI shims. Ungated (pcov is always bundled). Used at
/// startup and after a PHP install.
pub(crate) async fn refresh_pcov_and_shims(state: &DaemonState) {
    let dl = crate::php_install::ReqwestDownloader::new();
    crate::ext_install::ensure_pcov_for_installed(&state.dirs, &dl).await;
    reconcile_shims_now(state).await;
}

/// `restart php <ver>` - stop + ensure the version's FPM pool. Starts a stopped
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

/// `restart php` (no version) - restart every started pool (running or failed).
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

/// `uninstall php <ver>` - remove an installed version after safety checks.
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
                "PHP {version} is assigned to site(s): {} - reassign them first",
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
            message: format!("PHP {version} is the default - set another version as default first"),
        };
    }

    let _ = state.php_manager.lock().await.stop(version).await;
    let version_dir = state
        .dirs
        .data
        .join("php")
        .join(format!("php-{}.{}", version.major, version.minor));
    if let Err(e) = std::fs::remove_dir_all(&version_dir) {
        return internal(format!("failed to remove PHP {version}: {e}"));
    }
    let _ = std::fs::remove_file(crate::ext_install::pcov_so_path(&state.dirs, version));
    refresh_php_binaries(state).await;
    reconcile_shims_now(state).await;
    tracing::info!(version = %version, "uninstalled PHP");
    php_versions_response(state).await
}

/// `use <ver>` (global) - require the version installed, set the live default +
/// site fallback (`config.php.default`), persist, and repoint the `php` shim.
async fn set_default_php(version: yerd_core::PhpVersion, state: &DaemonState) -> Response {
    if !crate::php_install::cli_binary_path(&state.dirs, version).exists() {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("PHP {version} is not installed - run `yerd install php {version}`"),
        };
    }
    {
        let mut cfg_guard = state.config.lock().await;
        let mut new = cfg_guard.clone();
        new.php.default = version;
        if let Some(yerd_bin) = yerd_sibling() {
            if let Err(e) = crate::php_install::set_default_shim(&state.dirs, &yerd_bin) {
                return internal(format!("update php shim failed: {e}"));
            }
        } else {
            tracing::warn!("cannot locate the `yerd` binary; skipping php shim update");
        }
        if let Err(e) = new.save(&state.config_path) {
            return internal(format!("config save failed: {e}"));
        }
        *cfg_guard = new;
    }
    reconcile_shims_for(state).await;
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

/// Set the rootless HTTP/HTTPS fallback ports. Persisted to config; takes effect
/// on the next daemon restart (the client triggers it). Refused while a
/// privileged-port redirect is active - it is pinned to the current ports, so
/// changing them would break elevation until the user re-elevates.
async fn set_fallback_ports(http: u16, https: u16, state: &DaemonState) -> Response {
    let redirect_active = tokio::task::spawn_blocking(|| {
        use yerd_platform::PortRedirector;
        yerd_platform::ActivePortRedirector::new().is_active()
    })
    .await
    .unwrap_or(None);
    if redirect_active == Some(true) {
        return internal(
            "ports are elevated - remove the privileged-port redirect first (un-elevate ports), \
             change the ports, then re-elevate"
                .to_owned(),
        );
    }

    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    new.ports.fallback_http = http;
    new.ports.fallback_https = https;
    if let Err(e) = new.validate() {
        return internal(format!("config validation failed: {e}"));
    }
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *cfg_guard = new;
    tracing::info!(
        http,
        https,
        "set fallback ports (effective on next restart)"
    );
    Response::Ok
}

/// Set the embedded DNS responder port (`dns_port`). Persisted to config; takes
/// effect on the next daemon restart (the client triggers it). A zero port is
/// rejected explicitly here - unlike the web/mail/dumps ports, `dns_port == 0` is
/// a *valid* "ephemeral" value for in-process tests, so `validate()` permits it;
/// for a user-facing change a zero port (which the OS resolver could never target)
/// is meaningless.
async fn set_dns_port(port: u16, state: &DaemonState) -> Response {
    if port == 0 {
        return Response::Error {
            code: yerd_ipc::ErrorCode::Internal,
            message: "DNS port must be non-zero".to_owned(),
        };
    }
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    new.dns_port = port;
    if let Err(e) = new.validate() {
        return internal(format!("config validation failed: {e}"));
    }
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *cfg_guard = new;
    tracing::info!(port, "set DNS port (effective on next restart)");
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

/// Enable or disable the proxy's symlink-escape protection. Persisted to config
/// and mirrored into the shared `symlink_protection` atomic, so the proxy picks
/// it up on the next request without a daemon restart.
async fn set_symlink_protection(enabled: bool, state: &DaemonState) -> Response {
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    new.symlink_protection = enabled;
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *cfg_guard = new;
    state
        .symlink_protection
        .store(enabled, std::sync::atomic::Ordering::Relaxed);
    tracing::info!(enabled, "set symlink protection");
    Response::Ok
}

/// `set/unset php` - merge global PHP ini settings into the config and apply
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

    {
        let mut mgr = state.php_manager.lock().await;
        mgr.set_ini_settings(applied);
        for snap in mgr.snapshots() {
            if let Err(e) = mgr.restart(snap.version).await {
                tracing::warn!(version = %snap.version, error = %e, "failed to restart FPM pool after settings change");
            }
        }
    }
    write_cli_ini_now(state).await;
    tracing::info!("applied global PHP settings");
    php_versions_response(state).await
}

/// Register a custom extension for `version`: validate, load-probe, persist, then
/// load it into that version's FPM pool + CLI ini. Modeled on `set_php_settings`.
///
/// A cheap pre-probe duplicate check returns immediately when the extension is
/// already registered, avoiding a PHP spawn; the authoritative check under the
/// write lock still guards against a concurrent add.
async fn add_php_extension(
    version: yerd_core::PhpVersion,
    path: String,
    name: Option<String>,
    zend: bool,
    state: &DaemonState,
) -> Response {
    let php_bin = crate::php_install::cli_binary_path(&state.dirs, version);
    if !php_bin.exists() {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("PHP {version} is not installed - run `yerd install php {version}`"),
        };
    }
    let name = name
        .or_else(|| yerd_core::php_extensions::default_name_from_path(&path))
        .unwrap_or_default();
    if let Err(e) = yerd_core::php_extensions::validate_entry(&name, &path, zend) {
        return Response::Error {
            code: ErrorCode::InvalidPath,
            message: e.to_string(),
        };
    }

    if state
        .config
        .lock()
        .await
        .php
        .extensions
        .get(&version)
        .is_some_and(|list| list.iter().any(|e| e.name == name))
    {
        return Response::Error {
            code: ErrorCode::AlreadyExists,
            message: format!("an extension named {name} is already registered for PHP {version}"),
        };
    }

    let runner = yerd_php::TokioCommandRunner;
    if let Err(e) =
        yerd_php::probe_extension(&runner, &php_bin, std::path::Path::new(&path), zend).await
    {
        return Response::Error {
            code: ErrorCode::ExtensionLoadFailed,
            message: format!("extension failed to load into PHP {version}: {e}"),
        };
    }

    {
        let mut cfg_guard = state.config.lock().await;
        let mut new = cfg_guard.clone();
        let list = new.php.extensions.entry(version).or_default();
        if list.iter().any(|e| e.name == name) {
            return Response::Error {
                code: ErrorCode::AlreadyExists,
                message: format!(
                    "an extension named {name} is already registered for PHP {version}"
                ),
            };
        }
        list.push(yerd_config::ExtEntry { name, path, zend });
        if let Err(e) = new.validate() {
            return internal(format!("config validation failed: {e}"));
        }
        if let Err(e) = new.save(&state.config_path) {
            return internal(format!("config save failed: {e}"));
        }
        *cfg_guard = new;
    }
    apply_extensions(state, version).await;
    list_php_extensions(state).await
}

/// Remove a registered extension by name for `version`.
async fn remove_php_extension(
    version: yerd_core::PhpVersion,
    name: String,
    state: &DaemonState,
) -> Response {
    {
        let mut cfg_guard = state.config.lock().await;
        let mut new = cfg_guard.clone();
        let Some(list) = new.php.extensions.get_mut(&version) else {
            return Response::Error {
                code: ErrorCode::NotFound,
                message: format!("no extension named {name} registered for PHP {version}"),
            };
        };
        let before = list.len();
        list.retain(|e| e.name != name);
        if list.len() == before {
            return Response::Error {
                code: ErrorCode::NotFound,
                message: format!("no extension named {name} registered for PHP {version}"),
            };
        }
        if list.is_empty() {
            new.php.extensions.remove(&version);
        }
        if let Err(e) = new.save(&state.config_path) {
            return internal(format!("config save failed: {e}"));
        }
        *cfg_guard = new;
    }
    apply_extensions(state, version).await;
    list_php_extensions(state).await
}

/// List registered extensions across all versions, tagging each with whether its
/// `.so` currently exists on disk.
async fn list_php_extensions(state: &DaemonState) -> Response {
    let cfg = state.config.lock().await;
    let by_version = cfg
        .php
        .extensions
        .iter()
        .map(|(v, entries)| {
            let infos = entries
                .iter()
                .map(|e| yerd_ipc::PhpExtInfo {
                    name: e.name.clone(),
                    path: e.path.clone(),
                    zend: e.zend,
                    present: std::path::Path::new(&e.path).is_file(),
                })
                .collect();
            (*v, infos)
        })
        .collect();
    Response::PhpExtensions { by_version }
}

/// Push the config's extension registry into the live `PhpManager`, restart the
/// affected version's pool if it is currently running, and rewrite the per-version
/// CLI inis. Follows `set_php_settings`'s lock discipline: the config lock is
/// released before the manager lock is taken.
async fn apply_extensions(state: &DaemonState, affected: yerd_core::PhpVersion) {
    let ext_map = extension_load_map(state).await;
    {
        let mut mgr = state.php_manager.lock().await;
        mgr.set_extensions(ext_map);
        if mgr.snapshots().iter().any(|s| s.version == affected) {
            if let Err(e) = mgr.restart(affected).await {
                tracing::warn!(version = %affected, error = %e, "failed to restart FPM pool after extension change");
            }
        }
    }
    write_cli_ini_now(state).await;
}

/// Build the `PhpManager`'s extension map (`version -> [ExtLoad]`) from the
/// persisted config.
async fn extension_load_map(
    state: &DaemonState,
) -> std::collections::BTreeMap<yerd_core::PhpVersion, Vec<yerd_php::ExtLoad>> {
    let cfg = state.config.lock().await;
    cfg.php
        .extensions
        .iter()
        .map(|(v, entries)| {
            let loads = entries
                .iter()
                .map(|e| yerd_php::ExtLoad {
                    path: std::path::PathBuf::from(&e.path),
                    zend: e.zend,
                })
                .collect();
            (*v, loads)
        })
        .collect()
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
/// and swap the live router - **build-then-validate-then-commit** so a failed
/// mutation leaves disk and the live router untouched. A `Link`'s web-root
/// detection scan runs here, before `state.config` is locked, so a slow or
/// network-mounted project directory can't stall other mutating requests
/// that share the lock.
pub(crate) async fn handle_mutation(req: Request, state: &DaemonState) -> Response {
    let canonical = match &req {
        Request::Park { path } | Request::Link { path, .. } => match canonicalize_dir(path) {
            Ok(p) => Some(p),
            Err(resp) => return resp,
        },
        _ => None,
    };
    let link_web_subpath = match &req {
        Request::Link { .. } => canonical.as_deref().map(detect_web_subpath),
        _ => None,
    };

    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();

    let live_default = new.php.default;
    let applied = match &req {
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

    if let (Request::Link { name, .. }, Some(subpath)) = (&req, &link_web_subpath) {
        let name_lc = name.to_ascii_lowercase();
        if let Some(site) = new.linked.iter_mut().find(|s| s.name() == name_lc) {
            site.set_web_subpath(subpath);
        }
    }

    if let Err(e) = new.validate() {
        return internal(format!("config validation failed: {e}"));
    }

    let (candidate, candidate_wordpress, candidate_laravel) =
        match startup::build_router(&new, &state.dirs, &state.detect_cache) {
            Ok(r) => r,
            Err(DaemonError::Core(yerd_core::CoreError::DuplicateSite { name })) => {
                return Response::Error {
                    code: ErrorCode::AlreadyExists,
                    message: format!("duplicate site: {name}"),
                }
            }
            Err(e) => return internal(format!("router rebuild failed: {e}")),
        };

    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }

    *cfg_guard = new;
    let site_after = site_needing_url_sync(&req, &candidate);
    *state.router.write().await = candidate;
    *state.wordpress_sites.write().await = candidate_wordpress;
    *state.laravel_sites.write().await = candidate_laravel;
    drop(cfg_guard);

    state.watch_dirty.notify_one();

    if let Some(site) = site_after {
        crate::wordpress_url_sync::sync_site_url(&site, state).await;
    }

    tracing::info!(summary = %applied.summary, "applied mutation");
    Response::Ok
}

/// The post-mutation site to run [`crate::wordpress_url_sync::sync_site_url`]
/// against: `SetSecure` (which flips the scheme) plus every domain mutation
/// (each of which can change the primary domain a WordPress install should
/// advertise). `AddDomain` is included, not just `SetPrimaryDomain`/`ResetDomains`
/// /`RemoveDomain`: re-adding a previously-suppressed apex when the delta holds no
/// stored primary flips the derived primary back to the apex (`choose_primary`
/// prefers the apex over the first exact), so an add can change the primary too.
/// `sync_site_url` re-reads the just-rebuilt router's primary, so re-running it
/// when nothing actually changed is a harmless, idempotent no-op. `None` for
/// every other request kind. Looks `candidate` (the just-rebuilt router) up by
/// the *lowercased* site name, matching every other name-resolution site in
/// `mutate.rs` - the router is always keyed by the lowercased name (`Site`
/// lowercases at construction), so looking up an un-lowercased, user-typed name
/// (e.g. from `yerd secure MyWpSite`) would silently miss and skip the sync.
fn site_needing_url_sync(
    req: &Request,
    candidate: &yerd_core::SiteRouter,
) -> Option<yerd_core::Site> {
    let (Request::SetSecure { name, .. }
    | Request::AddDomain { name, .. }
    | Request::RemoveDomain { name, .. }
    | Request::SetPrimaryDomain { name, .. }
    | Request::ResetDomains { name }) = req
    else {
        return None;
    };
    candidate.get(&name.to_ascii_lowercase()).cloned()
}

/// Whether `url` is a loopback target on one of Yerd's **actively bound** proxy
/// ports - a request to such a proxy would forward straight back into Yerd,
/// re-resolve, and loop. Checked here rather than in the pure config layer
/// because the bound port is a runtime fact. A malformed URL returns `false` so
/// the mutation handler surfaces the precise parse error instead.
fn is_self_forward(url: &str, bound_ports: &[u16]) -> bool {
    let Ok(target) = yerd_core::UpstreamTarget::from_url_str(url) else {
        return false;
    };
    let loopback = target.host() == "localhost"
        || target
            .host()
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    loopback && bound_ports.contains(&target.port())
}

/// Reply to [`Request::ListProxies`]: whole-host proxies plus every per-site
/// path-prefix rule. Linked rules key by site name already; parked rules key by
/// document-root, which is resolved through the live router to the current site
/// name (mirroring `ListSites`) so the output round-trips through
/// `yerd proxy remove <site> <prefix>`. A parked docroot with no current site
/// falls back to the raw key.
async fn list_proxies(state: &DaemonState) -> Response {
    let cfg = state.config.lock().await;
    let router = state.router.read().await;
    let proxies = cfg
        .proxies
        .iter()
        .map(|p| yerd_ipc::ProxyEntry {
            name: p.name().to_owned(),
            target: p.target().to_string(),
            secure: p.secure(),
        })
        .collect();
    let mut rules = Vec::new();
    for (site, site_rules) in &cfg.proxy_rules.linked {
        for r in site_rules {
            rules.push(yerd_ipc::ProxyRuleEntry {
                site: site.clone(),
                prefix: r.prefix().to_owned(),
                target: r.target().to_string(),
            });
        }
    }
    for (docroot, site_rules) in &cfg.proxy_rules.parked {
        let site_name = router
            .iter()
            .find(|s| s.document_root().to_string_lossy().as_ref() == docroot.as_str())
            .map_or_else(|| docroot.clone(), |s| s.name().to_owned());
        for r in site_rules {
            rules.push(yerd_ipc::ProxyRuleEntry {
                site: site_name.clone(),
                prefix: r.prefix().to_owned(),
                target: r.target().to_string(),
            });
        }
    }
    Response::Proxies { proxies, rules }
}

/// Apply a group mutation (create/delete/reorder/assign). Groups are a
/// config-only organisational overlay that never affects routing, so this uses
/// the lighter clone → apply → validate → save → commit path (like
/// [`set_dns_port`]) - **no** router rebuild and **no** `watch_dirty` notify,
/// which would only provoke a needless parked-dir rescan.
async fn handle_group_mutation(req: Request, state: &DaemonState) -> Response {
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();

    // Groups ignore `default_php`, but `apply` still takes it; capture before the
    // mutable borrow of `new`.
    let live_default = new.php.default;
    let applied = match mutate::apply(
        &mut new,
        &*state.router.read().await,
        &req,
        None,
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

    if let Err(e) = new.validate() {
        return internal(format!("config validation failed: {e}"));
    }
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }

    *cfg_guard = new;
    tracing::info!(summary = %applied.summary, "applied group mutation");
    Response::Ok
}

/// Auto-detect the web subpath to serve for a project at `doc_root` (e.g.
/// `public` for Laravel). Shared by `SetWebRoot`'s auto-detect branch and
/// `Link`'s creation-time auto-detect.
fn detect_web_subpath(doc_root: &Path) -> String {
    yerd_core::detect(&yerd_platform::gather_project_signals(doc_root))
        .subpath
        .to_string_lossy()
        .into_owned()
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

    if let Some(site) = new.linked.iter_mut().find(|s| s.name() == name_lc) {
        let doc_root = site.document_root().to_path_buf();
        let rel = if let Some(p) = path {
            resolve_web_root_within(&doc_root, p)?
        } else {
            detect_web_subpath(&doc_root)
        };
        site.set_web_subpath(&rel);
        return Ok(mutate::Applied {
            summary: web_root_summary(&name_lc, &rel),
        });
    }

    if let Some(parked) = router.get(&name_lc) {
        let key = parked.document_root().to_string_lossy().into_owned();
        if let Some(p) = path {
            let rel = resolve_web_root_within(parked.document_root(), p)?;
            new.overrides.entry(key).or_default().web_root = Some(rel.clone());
            return Ok(mutate::Applied {
                summary: web_root_summary(&name_lc, &rel),
            });
        }
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

pub(crate) fn internal(message: String) -> Response {
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
    clippy::indexing_slicing,
    clippy::case_sensitive_file_extension_comparisons
)]
mod tests {
    use super::*;
    use yerd_core::{PhpVersion, RouterConfig, SiteRouter, Tld};
    use yerd_platform::PlatformDirs;

    use crate::test_support::state_in;

    #[test]
    fn self_forward_matches_only_loopback_on_bound_ports() {
        let bound = [8080, 8443];
        assert!(is_self_forward("http://127.0.0.1:8080", &bound));
        assert!(is_self_forward("https://localhost:8443", &bound));
        assert!(is_self_forward("http://[::1]:8080", &bound));
        assert!(!is_self_forward("http://127.0.0.1:3000", &bound));
        assert!(!is_self_forward("http://192.168.1.5:8080", &bound));
        assert!(!is_self_forward("http://example.com:8080", &bound));
        assert!(!is_self_forward("not-a-url", &bound));
    }

    #[test]
    fn bundle_contains_ca_matches_embedded_ca() {
        let ca = "-----BEGIN CERTIFICATE-----\nCA\n-----END CERTIFICATE-----";
        let bundle =
            format!("-----BEGIN CERTIFICATE-----\nROOT\n-----END CERTIFICATE-----\n{ca}\n");
        assert!(bundle_contains_ca(ca, &bundle));
        assert!(bundle_contains_ca(&format!("{ca}\n\n"), &bundle));
    }

    #[test]
    fn bundle_contains_ca_false_when_absent() {
        let ca = "-----BEGIN CERTIFICATE-----\nCA\n-----END CERTIFICATE-----";
        let bundle = "-----BEGIN CERTIFICATE-----\nOTHER\n-----END CERTIFICATE-----\n";
        assert!(!bundle_contains_ca(ca, bundle));
    }

    #[test]
    fn bundle_contains_ca_empty_ca_is_never_trusted() {
        assert!(!bundle_contains_ca("", "anything"));
        assert!(!bundle_contains_ca("   \n", "anything"));
    }

    #[test]
    fn site_needing_url_sync_finds_mixed_case_name() {
        let mut router = SiteRouter::new(RouterConfig::with_tld(Tld::new("test").unwrap()));
        router
            .insert(
                yerd_core::Site::linked("myblog", "/srv/myblog", PhpVersion::new(8, 3)).unwrap(),
            )
            .unwrap();
        let req = Request::SetSecure {
            name: "MyBlog".into(),
            secure: true,
        };
        let site = site_needing_url_sync(&req, &router);
        assert_eq!(site.map(|s| s.name().to_owned()), Some("myblog".to_owned()));
    }

    #[test]
    fn site_needing_url_sync_covers_all_domain_mutations() {
        let mut router = SiteRouter::new(RouterConfig::with_tld(Tld::new("test").unwrap()));
        router
            .insert(
                yerd_core::Site::linked("myblog", "/srv/myblog", PhpVersion::new(8, 3)).unwrap(),
            )
            .unwrap();
        for req in [
            Request::AddDomain {
                name: "MyBlog".into(),
                domain: "api.myblog.test".into(),
            },
            Request::RemoveDomain {
                name: "myblog".into(),
                domain: "corp.test".into(),
            },
            Request::SetPrimaryDomain {
                name: "MyBlog".into(),
                domain: "corp.test".into(),
            },
            Request::ResetDomains {
                name: "myblog".into(),
            },
        ] {
            assert_eq!(
                site_needing_url_sync(&req, &router).map(|s| s.name().to_owned()),
                Some("myblog".to_owned()),
                "{req:?} should trigger the WordPress URL sync"
            );
        }
    }

    #[test]
    fn site_needing_url_sync_none_for_non_domain_requests() {
        let router = SiteRouter::new(RouterConfig::with_tld(Tld::new("test").unwrap()));
        assert!(site_needing_url_sync(
            &Request::SetPhp {
                name: "myblog".into(),
                version: PhpVersion::new(8, 3),
            },
            &router
        )
        .is_none());
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

        match dispatch(Request::ListMails, &state).await {
            Response::Mails { mails } => assert!(mails.is_empty()),
            other => panic!("expected Mails, got {other:?}"),
        }

        state.mail_store.append(SAMPLE_EML).await.unwrap();
        let id = match dispatch(Request::ListMails, &state).await {
            Response::Mails { mails } => {
                assert_eq!(mails.len(), 1);
                assert_eq!(mails[0].subject, "Captured");
                mails[0].id.clone()
            }
            other => panic!("expected Mails, got {other:?}"),
        };

        match dispatch(Request::GetMail { id: id.clone() }, &state).await {
            Response::Mail { mail } => assert_eq!(mail.subject, "Captured"),
            other => panic!("expected Mail, got {other:?}"),
        }

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

        assert!(matches!(
            dispatch(Request::ClearMails, &state).await,
            Response::Ok
        ));
        match dispatch(Request::ListMails, &state).await {
            Response::Mails { mails } => assert!(mails.is_empty()),
            other => panic!("expected empty Mails, got {other:?}"),
        }
    }

    async fn status_mail(state: &DaemonState) -> yerd_ipc::MailStatus {
        match dispatch(Request::Status, state).await {
            Response::Status { report } => report.mail.expect("status should carry mail"),
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_status_includes_mail() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        let empty = status_mail(&state).await;
        assert!(empty.enabled);
        assert_eq!(empty.port, yerd_config::DEFAULT_MAIL_PORT);
        assert!(!empty.listening);
        assert_eq!(empty.count, 0);
        assert_eq!(empty.unread, 0);

        state
            .mail_store
            .append(b"From: a@b.c\r\nTo: d@e.f\r\nSubject: Hi\r\n\r\nbody\r\n")
            .await
            .unwrap();
        let seeded = status_mail(&state).await;
        assert_eq!(seeded.count, 1);
        assert_eq!(seeded.unread, 1);

        state
            .mail_store
            .mark_read(&["000000".to_string()])
            .await
            .unwrap();
        let read = status_mail(&state).await;
        assert_eq!(read.count, 1);
        assert_eq!(read.unread, 0);
    }

    #[tokio::test]
    async fn dispatch_set_mail_port_persists_and_rejects_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

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

        match dispatch(Request::ListSites, &state).await {
            Response::Sites { sites } => {
                let names: Vec<&str> = sites.iter().map(|e| e.site.name()).collect();
                assert_eq!(names, vec!["blog"]);
            }
            other => panic!("expected Sites, got {other:?}"),
        }
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
    async fn link_auto_detects_web_root_for_laravel() {
        let tmp = tempfile::tempdir().unwrap();
        let docroot = tmp.path().join("app");
        std::fs::create_dir_all(docroot.join("public")).unwrap();
        std::fs::write(docroot.join("artisan"), b"").unwrap();
        std::fs::write(docroot.join("public/index.php"), b"").unwrap();
        let state = state_in(tmp.path());

        let ok = dispatch(
            Request::Link {
                name: "app".into(),
                path: docroot,
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
    async fn link_plain_php_serves_document_root() {
        let tmp = tempfile::tempdir().unwrap();
        let docroot = tmp.path().join("plain");
        std::fs::create_dir_all(&docroot).unwrap();
        let state = state_in(tmp.path());

        let ok = dispatch(
            Request::Link {
                name: "plain".into(),
                path: docroot,
            },
            &state,
        )
        .await;
        assert!(matches!(ok, Response::Ok), "got {ok:?}");
        assert_eq!(
            web_subpath_of(&state, "plain").await,
            std::path::PathBuf::new()
        );
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
                .find(|s| s.site.name() == name)
                .unwrap_or_else(|| panic!("site {name} not found"))
                .site
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
                let blog = sites.iter().find(|s| s.site.name() == "blog").unwrap();
                assert_eq!(blog.site.php(), PhpVersion::new(8, 4));
                assert_eq!(blog.site.kind(), yerd_core::SiteKind::Parked);
            }
            other => panic!("expected Sites, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_parked_and_unpark_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let populated = tmp.path().join("populated");
        std::fs::create_dir_all(populated.join("blog")).unwrap();
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let state = state_in(tmp.path());
        dispatch(Request::Park { path: populated }, &state).await;
        dispatch(Request::Park { path: empty }, &state).await;

        let parked = match dispatch(Request::ListParked, &state).await {
            Response::Parked { paths } => paths,
            other => panic!("expected Parked, got {other:?}"),
        };
        assert_eq!(parked.len(), 2, "both roots registered: {parked:?}");
        let mut sorted = parked.clone();
        sorted.sort();
        assert_eq!(parked, sorted, "ListParked must be sorted");
        let populated_root = parked
            .iter()
            .find(|p| p.ends_with("populated"))
            .unwrap()
            .clone();

        let resp = dispatch(
            Request::Unpark {
                path: populated_root.clone(),
            },
            &state,
        )
        .await;
        assert!(matches!(resp, Response::Ok), "got {resp:?}");

        match dispatch(Request::ListParked, &state).await {
            Response::Parked { paths } => {
                assert_eq!(paths.len(), 1);
                assert!(paths[0].ends_with("empty"));
            }
            other => panic!("expected Parked, got {other:?}"),
        }
        match dispatch(Request::ListSites, &state).await {
            Response::Sites { sites } => {
                assert!(
                    sites.iter().all(|s| s.site.name() != "blog"),
                    "blog should be gone after un-park: {sites:?}"
                );
            }
            other => panic!("expected Sites, got {other:?}"),
        }

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
                let blog = sites.iter().find(|s| s.site.name() == "blog").unwrap();
                assert!(blog.site.secure());
                assert_eq!(blog.site.kind(), yerd_core::SiteKind::Parked);
            }
            other => panic!("expected Sites, got {other:?}"),
        }

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
                let blog = sites.iter().find(|s| s.site.name() == "blog").unwrap();
                assert!(!blog.site.secure());
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
                fallback_http,
                fallback_https,
                dns_port,
            } => {
                assert_eq!(dns_addr, state.dns_addr);
                assert_eq!(tld, "test");
                assert_eq!(ca_path, state.ca_path);
                assert_eq!(ca_fingerprint, state.ca_fingerprint.to_hex());
                assert_eq!(ca_fingerprint.len(), 64);
                assert_eq!(http_port, state.http.bound);
                assert_eq!(https_port, state.https.bound);
                assert_eq!(fallback_http, 8080);
                assert_eq!(fallback_https, 8443);
                assert_eq!(dns_port, state.config.lock().await.dns_port);
            }
            other => panic!("expected Info, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_set_dns_port_rejects_zero_and_persists_valid() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        match dispatch(Request::SetDnsPort { port: 0 }, &state).await {
            Response::Error { code, .. } => assert!(matches!(code, ErrorCode::Internal)),
            other => panic!("expected Error, got {other:?}"),
        }

        assert!(matches!(
            dispatch(Request::SetDnsPort { port: 5354 }, &state).await,
            Response::Ok
        ));
        assert_eq!(state.config.lock().await.dns_port, 5354);
        let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
        assert_eq!(reloaded.dns_port, 5354);
    }

    #[tokio::test]
    async fn dispatch_group_mutations_persist_without_router_churn() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        for name in ["Blog", "Shop"] {
            assert!(matches!(
                dispatch(Request::CreateGroup { name: name.into() }, &state).await,
                Response::Ok
            ));
        }
        assert!(matches!(
            dispatch(
                Request::SetSiteGroup {
                    site: "api".into(),
                    group: Some("Blog".into()),
                },
                &state,
            )
            .await,
            Response::Ok
        ));

        // ListGroups reflects the mutations in memory...
        match dispatch(Request::ListGroups, &state).await {
            Response::Groups { order, members } => {
                assert_eq!(order, vec!["Blog".to_string(), "Shop".to_string()]);
                assert_eq!(members.get("api").map(String::as_str), Some("Blog"));
            }
            other => panic!("expected Groups, got {other:?}"),
        }
        // ...and they persisted to disk.
        let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
        assert_eq!(
            reloaded.groups.order,
            vec!["Blog".to_string(), "Shop".to_string()]
        );
        assert_eq!(
            reloaded.groups.members.get("api").map(String::as_str),
            Some("Blog")
        );

        // Group mutations take the lighter commit path, so they must NOT signal
        // the parked-dir/router watcher (that would provoke a needless rescan).
        let not_notified = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            state.watch_dirty.notified(),
        )
        .await;
        assert!(
            not_notified.is_err(),
            "a group mutation must not notify watch_dirty"
        );

        // Contrast: a real site mutation DOES notify it - proving the probe works
        // and that the group path genuinely diverges from handle_mutation.
        let dir = tmp.path().join("sites");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(
            dispatch(Request::Park { path: dir }, &state).await,
            Response::Ok
        ));
        let notified = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            state.watch_dirty.notified(),
        )
        .await;
        assert!(
            notified.is_ok(),
            "a site mutation should notify watch_dirty"
        );
    }

    #[tokio::test]
    async fn dispatch_cached_update_status_uncached_reports_running_version() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        match dispatch(Request::CachedUpdateStatus, &state).await {
            Response::UpdateStatus {
                source,
                available,
                target,
                checked_at_epoch,
                ..
            } => {
                assert!(matches!(source, yerd_ipc::UpdateSource::Cached));
                assert!(!available);
                assert!(target.is_none());
                assert!(checked_at_epoch.is_none());
            }
            other => panic!("expected UpdateStatus, got {other:?}"),
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
                assert!(report.http.fell_back);
                assert_eq!(report.http.requested, 80);
                assert_eq!(report.http.bound, 8080);
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
                assert!(report.performed.is_empty());
                assert!(report
                    .manual
                    .iter()
                    .any(|d| d.severity == yerd_ipc::Severity::Fail));
            }
            other => panic!("expected DoctorFix, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_doctor_fix_rebuilds_missing_php_ca_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = state_in(tmp.path());
        std::fs::create_dir_all(&state.dirs.data).unwrap();
        let validity = yerd_tls::Validity::new(
            time::OffsetDateTime::now_utc() - time::Duration::days(1),
            time::OffsetDateTime::now_utc() + time::Duration::days(365),
        )
        .unwrap();
        let ca = yerd_tls::CertAuthority::generate(yerd_core::CA_COMMON_NAME, validity).unwrap();
        std::fs::write(&state.ca_path, ca.cert_pem()).unwrap();
        state.php_ca_bundle = Some(state.dirs.data.join("cacert.pem"));

        match dispatch(Request::DoctorFix, &state).await {
            Response::DoctorFix { report } => {
                let fix = report
                    .performed
                    .iter()
                    .find(|r| r.code == yerd_ipc::DiagnosisCode::PhpCaNotTrusted)
                    .expect("rebuild fix should have run");
                if fix.ok {
                    let bundle =
                        std::fs::read_to_string(state.dirs.data.join("cacert.pem")).unwrap();
                    assert!(bundle.contains(ca.cert_pem().trim()));
                }
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
                assert_eq!(default, PhpVersion::new(8, 3));
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
        let shim = state.dirs.data.join("bin").join("php");
        assert_eq!(
            std::fs::read_link(shim).unwrap(),
            yerd_sibling().expect("yerd sibling resolves in tests")
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
                let app = sites.iter().find(|s| s.site.name() == "app").unwrap();
                assert_eq!(app.site.php(), PhpVersion::new(8, 4));
            }
            other => panic!("expected Sites, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_php_settings_persists_validates_and_resets() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

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
                assert_eq!(
                    settings.get("display_errors").map(String::as_str),
                    Some("On")
                );
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }
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

    #[tokio::test]
    async fn add_php_extension_uninstalled_version_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let resp = dispatch(
            Request::AddPhpExtension {
                version: PhpVersion::new(8, 5),
                path: "/a/scrypt.so".to_string(),
                name: None,
                zend: false,
            },
            &state,
        )
        .await;
        assert!(matches!(
            resp,
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn add_php_extension_invalid_path_rejected_before_probe() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install(&state.dirs, PhpVersion::new(8, 5));
        let resp = dispatch(
            Request::AddPhpExtension {
                version: PhpVersion::new(8, 5),
                path: "relative/scrypt.so".to_string(),
                name: None,
                zend: false,
            },
            &state,
        )
        .await;
        assert!(matches!(
            resp,
            Response::Error {
                code: ErrorCode::InvalidPath,
                ..
            }
        ));
        assert!(state.config.lock().await.php.extensions.is_empty());
    }

    #[tokio::test]
    async fn remove_and_list_php_extensions() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        {
            let mut cfg = state.config.lock().await;
            let mut new = cfg.clone();
            new.php.extensions.insert(
                PhpVersion::new(8, 5),
                vec![yerd_config::ExtEntry {
                    name: "scrypt".to_string(),
                    path: "/a/scrypt.so".to_string(),
                    zend: false,
                }],
            );
            new.save(&state.config_path).unwrap();
            *cfg = new;
        }

        match dispatch(Request::ListPhpExtensions, &state).await {
            Response::PhpExtensions { by_version } => {
                let list = by_version.get(&PhpVersion::new(8, 5)).unwrap();
                assert_eq!(list.len(), 1);
                assert_eq!(list[0].name, "scrypt");
                assert!(!list[0].present, "missing .so should read as not present");
            }
            other => panic!("expected PhpExtensions, got {other:?}"),
        }

        match dispatch(
            Request::RemovePhpExtension {
                version: PhpVersion::new(8, 5),
                name: "nope".to_string(),
            },
            &state,
        )
        .await
        {
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            } => {}
            other => panic!("expected NotFound, got {other:?}"),
        }

        match dispatch(
            Request::RemovePhpExtension {
                version: PhpVersion::new(8, 5),
                name: "scrypt".to_string(),
            },
            &state,
        )
        .await
        {
            Response::PhpExtensions { by_version } => assert!(by_version.is_empty()),
            other => panic!("expected empty PhpExtensions, got {other:?}"),
        }
        assert!(state.config.lock().await.php.extensions.is_empty());
    }

    /// `ListPhp` annotates an installed minor from the (pre-seeded) update cache,
    /// with no network.
    #[tokio::test]
    async fn dispatch_list_php_surfaces_cached_update() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_build(&state.dirs, PhpVersion::new(8, 5), "8.5.6", 1);
        state
            .php_updates
            .write()
            .await
            .insert(PhpVersion::new(8, 5), ("8.5.7".to_owned(), 1));

        match dispatch(Request::ListPhp, &state).await {
            Response::PhpVersions { updates, .. } => {
                assert_eq!(updates.len(), 1);
                assert_eq!(updates[0].version, PhpVersion::new(8, 5));
                assert_eq!(updates[0].installed, "8.5.6-1");
                assert_eq!(updates[0].latest, "8.5.7-1");
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }
    }

    /// A legacy install (no `.yerd-revision`, so revision 0) is offered the
    /// c-ares-cutover rebuild of the *same* patch - the auto-heal contract.
    #[tokio::test]
    async fn dispatch_list_php_surfaces_revision_autoheal() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_patch(&state.dirs, PhpVersion::new(8, 5), "8.5.7");
        state
            .php_updates
            .write()
            .await
            .insert(PhpVersion::new(8, 5), ("8.5.7".to_owned(), 1));

        match dispatch(Request::ListPhp, &state).await {
            Response::PhpVersions { updates, .. } => {
                assert_eq!(updates.len(), 1);
                assert_eq!(updates[0].installed, "8.5.7");
                assert_eq!(updates[0].latest, "8.5.7-1");
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }
    }

    /// Same build (patch + revision) → no update annotation.
    #[tokio::test]
    async fn dispatch_list_php_no_update_when_cache_not_newer() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_build(&state.dirs, PhpVersion::new(8, 5), "8.5.6", 1);
        state
            .php_updates
            .write()
            .await
            .insert(PhpVersion::new(8, 5), ("8.5.6".to_owned(), 1));

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

    /// Fake downloader for the listing path: serves a signed `php.json` +
    /// `php.json.minisig`; anything else errors (the poll/available paths only
    /// fetch the manifest, not tarballs).
    struct ListingDl {
        manifest: String,
        minisig: String,
    }
    impl ListingDl {
        fn new(signed: &crate::test_support::SignedManifest) -> Self {
            Self {
                manifest: signed.manifest.clone(),
                minisig: signed.minisig.clone(),
            }
        }
    }
    #[async_trait::async_trait]
    impl yerd_php::Downloader for ListingDl {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            if url.ends_with(".minisig") {
                Ok(self.minisig.clone().into_bytes())
            } else if url.ends_with("php.json") {
                Ok(self.manifest.clone().into_bytes())
            } else {
                Err(yerd_php::DownloadError::Transport {
                    url: url.to_owned(),
                    reason: "unexpected".into(),
                })
            }
        }
    }

    /// Build + sign a `php.json` with the given `(php, minor, revision)` builds
    /// for the host platform. Tarball shas are placeholders (`"00"`) - the poll /
    /// available paths never download tarballs.
    fn signed_listing(builds: &[(&str, &str, u32)]) -> crate::test_support::SignedManifest {
        let (os, arch) = yerd_php::current_os_arch().unwrap();
        let entries: Vec<String> = builds
            .iter()
            .map(|(php, minor, rev)| {
                format!(
                    r#"{{ "php": "{php}", "minor": "{minor}", "os": "{os}", "arch": "{arch}", "revision": {rev},
                       "cli": {{ "file": "php-{php}-{rev}-cli-{os}-{arch}.tar.gz", "sha256": "00", "size": 1 }},
                       "fpm": {{ "file": "php-{php}-{rev}-fpm-{os}-{arch}.tar.gz", "sha256": "00", "size": 1 }} }}"#,
                    os = os.as_str(),
                    arch = arch.as_str(),
                )
            })
            .collect();
        let manifest = format!("{{ \"schema\": 1, \"builds\": [{}] }}", entries.join(","));
        crate::test_support::sign_manifest(&manifest)
    }

    /// Like `fake_install_patch` but also writes the `.yerd-revision` marker.
    fn fake_install_build(dirs: &PlatformDirs, v: PhpVersion, full: &str, revision: u32) {
        fake_install_patch(dirs, v, full);
        let base = dirs
            .data
            .join("php")
            .join(format!("php-{}.{}", v.major, v.minor));
        std::fs::write(base.join(".yerd-revision"), revision.to_string()).unwrap();
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
        fake_install_build(&state.dirs, PhpVersion::new(8, 5), "8.5.6", 1);
        let signed = signed_listing(&[("8.5.9", "8.5", 1)]);

        crate::php_updates::poll_and_refresh(&state, &ListingDl::new(&signed), &signed.public_key)
            .await;

        assert_eq!(
            state
                .php_updates
                .read()
                .await
                .get(&PhpVersion::new(8, 5))
                .cloned(),
            Some(("8.5.9".to_owned(), 1))
        );
    }

    #[tokio::test]
    async fn poll_and_refresh_is_failure_tolerant() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_build(&state.dirs, PhpVersion::new(8, 5), "8.5.6", 1);
        state
            .php_updates
            .write()
            .await
            .insert(PhpVersion::new(8, 5), ("8.5.6".to_owned(), 1));

        crate::php_updates::poll_and_refresh(
            &state,
            &FailingDl,
            yerd_update::PHP_LISTING_PUBLIC_KEY,
        )
        .await;

        assert_eq!(
            state
                .php_updates
                .read()
                .await
                .get(&PhpVersion::new(8, 5))
                .cloned(),
            Some(("8.5.6".to_owned(), 1))
        );
    }

    /// A validly-signed but unknown-schema manifest must NOT wipe a good cache:
    /// resolve fails schema-wide, so the poll aborts without overwriting.
    #[tokio::test]
    async fn poll_and_refresh_keeps_cache_on_unknown_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_build(&state.dirs, PhpVersion::new(8, 5), "8.5.6", 1);
        state
            .php_updates
            .write()
            .await
            .insert(PhpVersion::new(8, 5), ("8.5.7".to_owned(), 1));
        let signed = crate::test_support::sign_manifest(r#"{ "schema": 2, "builds": [] }"#);

        crate::php_updates::poll_and_refresh(&state, &ListingDl::new(&signed), &signed.public_key)
            .await;

        assert_eq!(
            state
                .php_updates
                .read()
                .await
                .get(&PhpVersion::new(8, 5))
                .cloned(),
            Some(("8.5.7".to_owned(), 1)),
            "a bad-schema manifest must not clear the previously-cached update"
        );
    }

    /// Fake GitHub Releases downloader: returns the canned JSON for the first
    /// page (the only page fetched, since the body has < 100 entries). The poll
    /// loop stops after a short page.
    struct ReleasesDl(&'static str);
    #[async_trait::async_trait]
    impl yerd_php::Downloader for ReleasesDl {
        async fn download(&self, _url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            Ok(self.0.as_bytes().to_vec())
        }
    }

    // A tiny releases payload. Far-future versions so the target is always newer
    // than the daemon's compiled `current` version, regardless of the build. The
    // unparsable `nightly-garbage` tag must be skipped.
    const RELEASES_JSON: &str = r#"[
        {"tag_name":"v99.1.0-rc.1","prerelease":true,"draft":false,"assets":[]},
        {"tag_name":"v99.0.1","prerelease":false,"draft":false,"assets":[]},
        {"tag_name":"v99.0.0","prerelease":false,"draft":false,"assets":[]},
        {"tag_name":"nightly-garbage","prerelease":true,"draft":false,"assets":[]}
    ]"#;

    #[tokio::test]
    async fn check_update_reports_both_channel_latests_live() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let resp = crate::self_update::check_update(None, &state, &ReleasesDl(RELEASES_JSON)).await;
        match resp {
            Response::UpdateStatus {
                latest_stable,
                latest_edge,
                channel,
                source,
                ..
            } => {
                assert_eq!(latest_stable.as_deref(), Some("99.0.1"));
                assert_eq!(latest_edge.as_deref(), Some("99.1.0-rc.1"));
                assert_eq!(channel, yerd_ipc::Channel::Stable);
                assert_eq!(source, yerd_ipc::UpdateSource::Live);
            }
            other => panic!("expected UpdateStatus, got {other:?}"),
        }
        assert_eq!(state.yerd_update.read().await.len(), 3);
    }

    #[tokio::test]
    async fn check_update_edge_override_selects_prerelease_target() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let resp = crate::self_update::check_update(
            Some(yerd_ipc::Channel::Edge),
            &state,
            &ReleasesDl(RELEASES_JSON),
        )
        .await;
        match resp {
            Response::UpdateStatus {
                channel,
                target,
                available,
                ..
            } => {
                assert_eq!(channel, yerd_ipc::Channel::Edge);
                assert_eq!(target.as_deref(), Some("99.1.0-rc.1"));
                assert!(available);
            }
            other => panic!("expected UpdateStatus, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn check_update_falls_back_to_cache_when_offline() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        crate::self_update::poll_and_refresh(&state, &ReleasesDl(RELEASES_JSON)).await;
        let resp = crate::self_update::check_update(None, &state, &FailingDl).await;
        match resp {
            Response::UpdateStatus {
                latest_stable,
                source,
                ..
            } => {
                assert_eq!(source, yerd_ipc::UpdateSource::Cached);
                assert_eq!(latest_stable.as_deref(), Some("99.0.1"));
            }
            other => panic!("expected UpdateStatus, got {other:?}"),
        }
    }

    // Known-good minisign fixture (the `minisign-verify` crate's published test
    // vector: a prehashed signature of the bytes `b"test"`).
    const SIG_PUBKEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const SIG_FIXTURE: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1556193335\tfile:test\ny/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==";

    /// Fake downloader for the full stage flow. Serves the releases JSON for the
    /// API URL; the signed fixture bytes (`b"test"`) for any artifact URL; the
    /// fixture signature for any `.sig` URL; and a matching `SHA256SUMS`.
    struct StageDl {
        releases: String,
        sums: String,
    }
    #[async_trait::async_trait]
    impl yerd_php::Downloader for StageDl {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            if url.contains("api.github.com") {
                Ok(self.releases.clone().into_bytes())
            } else if url.ends_with("SHA256SUMS") {
                Ok(self.sums.clone().into_bytes())
            } else if url.ends_with(".sig") {
                Ok(SIG_FIXTURE.as_bytes().to_vec())
            } else {
                Ok(b"test".to_vec())
            }
        }
    }

    #[tokio::test]
    async fn stage_update_downloads_verifies_and_writes_artifact() {
        if !matches!(
            yerd_update::Platform::current(),
            yerd_update::Platform::MacOsAarch64
                | yerd_update::Platform::LinuxX86_64
                | yerd_update::Platform::LinuxAarch64
        ) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        let mac = "Yerd_MacOS_AppleSilicon_v99-0-1.app.tar.gz";
        let deb = "Yerd_Linux_x86_64_v99-0-1.deb";
        let arm = "Yerd_Linux_Arm64_v99-0-1.deb";
        let pkg = "Yerd_Linux_x86_64_v99-0-1.pkg.tar.zst";
        let pkg_arm = "Yerd_Linux_Arm64_v99-0-1.pkg.tar.zst";
        let rpm = "Yerd_Linux_x86_64_v99-0-1.rpm";
        let rpm_arm = "Yerd_Linux_Arm64_v99-0-1.rpm";
        let releases = format!(
            r#"[{{"tag_name":"v99.0.1","prerelease":false,"draft":false,"assets":[
                {{"name":"{mac}","browser_download_url":"https://h/{mac}","size":4}},
                {{"name":"{mac}.sig","browser_download_url":"https://h/{mac}.sig","size":1}},
                {{"name":"{deb}","browser_download_url":"https://h/{deb}","size":4}},
                {{"name":"{deb}.sig","browser_download_url":"https://h/{deb}.sig","size":1}},
                {{"name":"{arm}","browser_download_url":"https://h/{arm}","size":4}},
                {{"name":"{arm}.sig","browser_download_url":"https://h/{arm}.sig","size":1}},
                {{"name":"{pkg}","browser_download_url":"https://h/{pkg}","size":4}},
                {{"name":"{pkg}.sig","browser_download_url":"https://h/{pkg}.sig","size":1}},
                {{"name":"{pkg_arm}","browser_download_url":"https://h/{pkg_arm}","size":4}},
                {{"name":"{pkg_arm}.sig","browser_download_url":"https://h/{pkg_arm}.sig","size":1}},
                {{"name":"{rpm}","browser_download_url":"https://h/{rpm}","size":4}},
                {{"name":"{rpm}.sig","browser_download_url":"https://h/{rpm}.sig","size":1}},
                {{"name":"{rpm_arm}","browser_download_url":"https://h/{rpm_arm}","size":4}},
                {{"name":"{rpm_arm}.sig","browser_download_url":"https://h/{rpm_arm}.sig","size":1}},
                {{"name":"SHA256SUMS","browser_download_url":"https://h/SHA256SUMS","size":1}}
            ]}}]"#
        );
        let h = yerd_update::sha256_hex(b"test");
        let sums = format!(
            "{h}  {mac}\n{h}  {deb}\n{h}  {arm}\n{h}  {pkg}\n{h}  {pkg_arm}\n{h}  {rpm}\n{h}  {rpm_arm}\n"
        );
        let dl = StageDl { releases, sums };

        let resp = crate::self_update::stage_update(None, &state, &dl, SIG_PUBKEY).await;
        match resp {
            Response::Staged {
                path,
                version,
                kind,
            } => {
                assert_eq!(version, "99.0.1");
                let p = std::path::Path::new(&path);
                assert!(p.exists(), "staged file should exist at {path}");
                assert_eq!(std::fs::read(p).unwrap(), b"test");
                let (expected_kind, expected_name) = match (
                    yerd_update::Platform::current(),
                    yerd_update::PkgFormat::current(),
                ) {
                    (yerd_update::Platform::MacOsAarch64, _) => {
                        (yerd_ipc::StagedArtifact::AppTarGz, mac)
                    }
                    (yerd_update::Platform::LinuxX86_64, yerd_update::PkgFormat::Deb) => {
                        (yerd_ipc::StagedArtifact::Deb, deb)
                    }
                    (yerd_update::Platform::LinuxX86_64, yerd_update::PkgFormat::Pacman) => {
                        (yerd_ipc::StagedArtifact::Pacman, pkg)
                    }
                    (yerd_update::Platform::LinuxAarch64, yerd_update::PkgFormat::Deb) => {
                        (yerd_ipc::StagedArtifact::Deb, arm)
                    }
                    (yerd_update::Platform::LinuxAarch64, yerd_update::PkgFormat::Pacman) => {
                        (yerd_ipc::StagedArtifact::Pacman, pkg_arm)
                    }
                    (yerd_update::Platform::LinuxX86_64, yerd_update::PkgFormat::Rpm) => {
                        (yerd_ipc::StagedArtifact::Rpm, rpm)
                    }
                    (yerd_update::Platform::LinuxAarch64, yerd_update::PkgFormat::Rpm) => {
                        (yerd_ipc::StagedArtifact::Rpm, rpm_arm)
                    }
                    (other, _) => panic!("unexpected platform for fixture: {other:?}"),
                };
                assert_eq!(kind, expected_kind);
                assert_eq!(
                    p.file_name().and_then(|n| n.to_str()),
                    Some(expected_name),
                    "staged basename should be the current platform+format's asset"
                );
            }
            other => panic!("expected Staged, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stage_update_rejects_verification_failure_and_writes_nothing() {
        if !matches!(
            yerd_update::Platform::current(),
            yerd_update::Platform::MacOsAarch64
                | yerd_update::Platform::LinuxX86_64
                | yerd_update::Platform::LinuxAarch64
        ) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let mac = "Yerd_MacOS_AppleSilicon_v99-0-1.app.tar.gz";
        let deb = "Yerd_Linux_x86_64_v99-0-1.deb";
        let arm = "Yerd_Linux_Arm64_v99-0-1.deb";
        let pkg = "Yerd_Linux_x86_64_v99-0-1.pkg.tar.zst";
        let pkg_arm = "Yerd_Linux_Arm64_v99-0-1.pkg.tar.zst";
        let releases = format!(
            r#"[{{"tag_name":"v99.0.1","prerelease":false,"draft":false,"assets":[
                {{"name":"{mac}","browser_download_url":"https://h/{mac}","size":4}},
                {{"name":"{mac}.sig","browser_download_url":"https://h/{mac}.sig","size":1}},
                {{"name":"{deb}","browser_download_url":"https://h/{deb}","size":4}},
                {{"name":"{deb}.sig","browser_download_url":"https://h/{deb}.sig","size":1}},
                {{"name":"{arm}","browser_download_url":"https://h/{arm}","size":4}},
                {{"name":"{arm}.sig","browser_download_url":"https://h/{arm}.sig","size":1}},
                {{"name":"{pkg}","browser_download_url":"https://h/{pkg}","size":4}},
                {{"name":"{pkg}.sig","browser_download_url":"https://h/{pkg}.sig","size":1}},
                {{"name":"{pkg_arm}","browser_download_url":"https://h/{pkg_arm}","size":4}},
                {{"name":"{pkg_arm}.sig","browser_download_url":"https://h/{pkg_arm}.sig","size":1}},
                {{"name":"SHA256SUMS","browser_download_url":"https://h/SHA256SUMS","size":1}}
            ]}}]"#
        );
        let bad = "0".repeat(64);
        let sums =
            format!("{bad}  {mac}\n{bad}  {deb}\n{bad}  {arm}\n{bad}  {pkg}\n{bad}  {pkg_arm}\n");
        let dl = StageDl { releases, sums };
        match crate::self_update::stage_update(None, &state, &dl, SIG_PUBKEY).await {
            Response::Error { .. } => {}
            other => panic!("expected Error on checksum mismatch, got {other:?}"),
        }
        assert!(
            !state.dirs.cache.join("update").join(mac).exists()
                && !state.dirs.cache.join("update").join(deb).exists()
                && !state.dirs.cache.join("update").join(arm).exists()
                && !state.dirs.cache.join("update").join(pkg).exists()
                && !state.dirs.cache.join("update").join(pkg_arm).exists(),
            "must not write an artifact when verification fails"
        );
    }

    #[tokio::test]
    async fn set_update_channel_persists_to_config() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert_eq!(
            dispatch(
                Request::SetUpdateChannel {
                    channel: yerd_ipc::Channel::Edge,
                },
                &state,
            )
            .await,
            Response::Ok
        );
        assert_eq!(state.config.lock().await.update_channel, "edge");
        let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
        assert_eq!(reloaded.update_channel, "edge");
    }

    #[tokio::test]
    async fn set_symlink_protection_persists_config_and_updates_live_atomic() {
        use std::sync::atomic::Ordering::Relaxed;
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert!(state.symlink_protection.load(Relaxed), "seeded protected");

        assert_eq!(
            dispatch(Request::SetSymlinkProtection { enabled: false }, &state,).await,
            Response::Ok
        );
        assert!(
            !state.config.lock().await.symlink_protection,
            "in-memory config off"
        );
        assert!(!state.symlink_protection.load(Relaxed), "live atomic off");
        let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
        assert!(!reloaded.symlink_protection, "persisted config off");
        assert!(
            !build_status_report(&state).await.symlink_protection,
            "status report off"
        );

        assert_eq!(
            dispatch(Request::SetSymlinkProtection { enabled: true }, &state).await,
            Response::Ok
        );
        assert!(
            state.symlink_protection.load(Relaxed),
            "live atomic back on"
        );
        let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
        assert!(reloaded.symlink_protection, "persisted config back on");
        assert!(
            build_status_report(&state).await.symlink_protection,
            "status report back on"
        );
    }

    #[tokio::test]
    async fn available_php_lists_distribution_minors_and_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_patch(&state.dirs, PhpVersion::new(8, 5), "8.5.6");
        let signed = signed_listing(&[("8.3.20", "8.3", 1), ("8.5.9", "8.5", 1)]);

        match available_php_with(&state, &ListingDl::new(&signed), &signed.public_key).await {
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

        match available_php_with(&state, &FailingDl, yerd_update::PHP_LISTING_PUBLIC_KEY).await {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::Internal),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    // ---------- pure helpers ----------

    #[test]
    fn pools_needing_restart_only_targets_active_updated_minors() {
        let active: std::collections::HashSet<PhpVersion> =
            [PhpVersion::new(8, 4), PhpVersion::new(8, 5)]
                .into_iter()
                .collect();
        let updated = [PhpVersion::new(8, 5), PhpVersion::new(8, 3)];
        assert_eq!(
            pools_needing_restart(&active, &updated),
            vec![PhpVersion::new(8, 5)]
        );
        assert!(pools_needing_restart(&active, &[]).is_empty());
        assert!(pools_needing_restart(&std::collections::HashSet::new(), &updated).is_empty());
    }

    #[test]
    fn map_pool_state_maps_both_variants() {
        assert_eq!(
            map_pool_state(yerd_php::PoolRunState::Running),
            yerd_ipc::PoolRunState::Running
        );
        assert_eq!(
            map_pool_state(yerd_php::PoolRunState::Failed),
            yerd_ipc::PoolRunState::Failed
        );
    }

    #[test]
    fn load_to_centi_clamps_and_rounds() {
        assert_eq!(load_to_centi(0.0), 0);
        assert_eq!(load_to_centi(-5.0), 0, "negative clamps to 0");
        assert_eq!(load_to_centi(1.234), 123, "rounded to hundredths");
        assert_eq!(load_to_centi(f64::from(u32::MAX)), u32::MAX, "saturates");
    }

    #[tokio::test]
    async fn installed_versions_empty_then_lists_fake_install() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert!(installed_versions(&state).is_empty());
        fake_install(&state.dirs, PhpVersion::new(8, 4));
        fake_install(&state.dirs, PhpVersion::new(8, 3));
        let versions = installed_versions(&state);
        assert_eq!(versions, vec![PhpVersion::new(8, 3), PhpVersion::new(8, 4)]);
    }

    #[test]
    fn path_needs_setup_no_tools_is_some_false() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        #[cfg(unix)]
        assert_eq!(path_needs_setup(&state), Some(false));
        #[cfg(not(unix))]
        assert_eq!(path_needs_setup(&state), None);
    }

    // ---------- additional `dispatch` arms ----------

    #[tokio::test]
    async fn dispatch_list_services_reports_all_engines_uninstalled() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match dispatch(Request::ListServices, &state).await {
            Response::Services { services } => {
                assert!(!services.is_empty(), "all engines enumerated");
                assert!(
                    services.iter().all(|s| s.installed_versions.is_empty()),
                    "{services:?}"
                );
            }
            other => panic!("expected Services, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_restart_all_php_no_pools_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert!(matches!(
            dispatch(Request::RestartAllPhp, &state).await,
            Response::Ok
        ));
    }

    #[tokio::test]
    async fn dispatch_set_fallback_ports_validates_and_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        match dispatch(
            Request::SetFallbackPorts {
                http: 9000,
                https: 9000,
            },
            &state,
        )
        .await
        {
            Response::Error { code, .. } => assert!(matches!(code, ErrorCode::Internal)),
            other => panic!("expected Error, got {other:?}"),
        }

        match dispatch(
            Request::SetFallbackPorts {
                http: 8081,
                https: 8444,
            },
            &state,
        )
        .await
        {
            Response::Ok => {
                {
                    let cfg = state.config.lock().await;
                    assert_eq!(cfg.ports.fallback_http, 8081);
                    assert_eq!(cfg.ports.fallback_https, 8444);
                }
                let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
                assert_eq!(reloaded.ports.fallback_http, 8081);
                assert_eq!(reloaded.ports.fallback_https, 8444);
            }
            Response::Error { code, message } => {
                assert!(matches!(code, ErrorCode::Internal));
                assert!(message.contains("elevated"), "{message}");
                let cfg = state.config.lock().await;
                assert_eq!(cfg.ports.fallback_http, 8080);
                assert_eq!(cfg.ports.fallback_https, 8443);
            }
            other => panic!("expected Ok or elevated Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_delete_mails_empty_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert!(matches!(
            dispatch(Request::DeleteMails { ids: vec![] }, &state).await,
            Response::Ok
        ));
    }

    // ---------- dump-server arms routed through `dispatch` ----------

    #[tokio::test]
    async fn dispatch_dumps_status_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        assert!(matches!(
            dispatch(Request::SetDumpsPersist { persist: true }, &state).await,
            Response::Ok
        ));
        assert!(state.config.lock().await.dumps.persist);

        assert!(matches!(
            dispatch(
                Request::SetDumpFeature {
                    feature: "queries".into(),
                    enabled: false,
                },
                &state,
            )
            .await,
            Response::Ok
        ));
        match dispatch(
            Request::SetDumpFeature {
                feature: "nope".into(),
                enabled: true,
            },
            &state,
        )
        .await
        {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::NotFound),
            other => panic!("expected NotFound, got {other:?}"),
        }

        match dispatch(Request::DumpsStatus, &state).await {
            Response::DumpsStatus {
                persist, features, ..
            } => {
                assert!(persist);
                assert_eq!(features.get("queries"), Some(&false));
            }
            other => panic!("expected DumpsStatus, got {other:?}"),
        }

        match dispatch(Request::ListDumps { since_id: 0 }, &state).await {
            Response::Dumps { events, .. } => assert!(events.is_empty()),
            other => panic!("expected Dumps, got {other:?}"),
        }
        assert!(matches!(
            dispatch(Request::ClearDumps, &state).await,
            Response::Ok
        ));
        assert!(matches!(
            dispatch(Request::DeleteDump { id: 42 }, &state).await,
            Response::Ok
        ));
    }
}
