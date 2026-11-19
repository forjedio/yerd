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
            Request::InstallNodeStreamed { version, update } => {
                install_node_streamed(version, update, state.clone()).await
            }
            Request::InstallCloudflaredStreamed => {
                crate::tunnel::install_cloudflared_streamed(state.clone()).await
            }
            Request::CloudflaredLogin => crate::tunnel::named::login_streamed(state.clone()).await,
            Request::StartQuickTunnel { site } => {
                crate::tunnel::start_quick_tunnel(&site, &state).await
            }
            Request::StartNamedTunnel => crate::tunnel::named::start(&state).await,
            Request::SetSiteTunnel { site, hostname } => {
                crate::tunnel::named::set_site_hostname(&site, hostname.as_deref(), &state).await
            }
            Request::InstallPhpStreamed {
                version,
                confirm_legacy,
            } => install_php_streamed(version, confirm_legacy, state.clone()).await,
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
            let favourites = state.favourite_sites.read().await.clone();
            let router = state.router.read().await;
            let tld = router.config().tld().to_owned();
            let wordpress_sites = state.wordpress_sites.read().await;
            let laravel_sites = state.laravel_sites.read().await;
            let framework_sites = state.framework_sites.read().await;
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
                        favourite: favourites.contains(&name.to_ascii_lowercase()),
                        framework: framework_sites.get(name).copied(),
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
                lan_ip: state.lan_ip,
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
        | Request::RemoveProxyRule { .. }
        | Request::AddRouteRule { .. }
        | Request::RemoveRouteRule { .. }
        | Request::SetResponseHeader { .. }
        | Request::RemoveResponseHeader { .. } => handle_mutation(req, state).await,
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
        Request::ListRoutes => list_routes(state).await,
        Request::ListResponseHeaders => list_response_headers(state).await,
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
        | Request::SetSiteFavourite { .. }
        | Request::RenameGroup { .. } => handle_group_mutation(req, state).await,
        Request::ListPhp => php_versions_response(state).await,
        Request::InstallPhp {
            version,
            confirm_legacy,
        } => install_php(version, confirm_legacy, state).await,
        Request::SetDefaultPhp { version } => set_default_php(version, state).await,
        Request::CheckPhpUpdates => {
            let dl = crate::network::downloader(state).await;
            crate::php_updates::poll_and_refresh(state, &dl, yerd_update::PHP_LISTING_PUBLIC_KEY)
                .await;
            php_versions_response(state).await
        }
        Request::UpdatePhp { version } => update_php(version, state).await,
        Request::AvailablePhp => available_php_response(state).await,
        Request::SetPhpSettings { settings } => set_php_settings(settings, state).await,
        Request::SetPhpVersionSettings { version, settings } => {
            set_php_version_settings(version, settings, state).await
        }
        Request::SetPhpDirectives {
            version,
            directives,
        } => set_php_directives(version, directives, state).await,
        Request::SetPhpPoolSettings { version, settings } => {
            set_php_pool_settings(version, settings, state).await
        }
        Request::SetXdebug {
            version,
            enabled,
            modes,
            start,
        } => {
            crate::xdebug::set_xdebug(
                state,
                crate::xdebug::XdebugRequest {
                    version,
                    enabled,
                    modes,
                    start,
                },
            )
            .await
        }
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
        Request::Diagnose => {
            let outbound = crate::network::doctor_probe(state).await;
            Response::Diagnoses {
                items: yerd_doctor::diagnose(
                    &build_status_report(state).await,
                    path_needs_setup(state).await,
                    &crate::services::local_override_files(&state.dirs),
                    Some(&outbound),
                ),
            }
        }
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
            let dl = crate::network::downloader(state).await;
            crate::services::available_services(state, &dl).await
        }
        Request::AvailableWordpressVersions => {
            let dl = crate::network::downloader(state).await;
            crate::wordpress_versions::available_versions(state, &dl).await
        }
        Request::MintWordpressLoginToken { site } => {
            crate::wordpress_login::mint_wordpress_login_token(&site, state).await
        }
        Request::WordpressAdminUsers { site } => {
            crate::wordpress_users::admin_users(&site, state).await
        }
        Request::InstallService { service, version } => {
            let dl = crate::network::downloader(state).await;
            let response = crate::services::install_service(&service, &version, state, &dl).await;
            reconcile_client_shims_now(state).await;
            response
        }
        Request::UninstallService {
            service,
            version,
            purge,
        } => {
            let response =
                crate::services::uninstall_service(&service, &version, purge, state).await;
            reconcile_client_shims_now(state).await;
            response
        }
        Request::StartService { service } => crate::services::start_service(&service, state).await,
        Request::StopService { service } => crate::services::stop_service(&service, state).await,
        Request::RestartService { service } => {
            crate::services::restart_service(&service, state).await
        }
        Request::SetServicePort { service, port } => {
            crate::services::set_service_port(&service, port, state).await
        }
        Request::SetServiceOverrides { service, overrides } => {
            crate::services::set_service_overrides(&service, overrides, state).await
        }
        Request::ServiceOverrides { service } => {
            crate::services::service_overrides(&service, state).await
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
            let dl = crate::network::downloader(state).await;
            let response = crate::services::add_service(
                &type_id,
                site.as_deref(),
                port,
                version.as_deref(),
                autostart,
                state,
                &dl,
            )
            .await;
            reconcile_client_shims_now(state).await;
            response
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
            let dl = crate::network::downloader(state).await;
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
            let dl = crate::network::downloader(state).await;
            let response =
                crate::services::change_service_version(&service, &version, state, &dl).await;
            reconcile_client_shims_now(state).await;
            response
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
        Request::SetMcpEnabled { enabled } => set_mcp_enabled(enabled, state).await,
        Request::GetNetworkSettings => crate::network::get_settings(state).await,
        Request::SetNetworkSettings {
            mode,
            http_proxy,
            https_proxy,
            no_proxy,
        } => crate::network::set_settings(state, mode, http_proxy, https_proxy, no_proxy).await,
        Request::TestNetwork => crate::network::test_network(state).await,
        Request::SetLanEnabled { enabled } => set_lan_enabled(enabled, state).await,
        Request::MintRemoteSetupCode => mint_remote_setup_code(state).await,
        Request::TrustBrowsers { uninstall } => trust_browsers(uninstall, state).await,
        Request::ListTools => Response::Tools {
            tools: list_tools_with_external(state).await,
        },
        Request::InstallTool { tool } => install_tool(&tool, state).await,
        Request::InstallNode { version } => install_node(&version, state).await,
        Request::SetNodeVersion { version, site } => {
            set_node_version(&version, site.as_deref(), state).await
        }
        Request::ListNode => node_versions_response(state).await,
        Request::AvailableNode => available_node_response(state).await,
        Request::UninstallNode { version } => uninstall_node(&version, state).await,
        Request::ClearNodeVersion { site } => clear_node_version(&site, state).await,
        Request::NodeSite { site } => node_site_response(&site, state).await,
        Request::NodeSites => node_sites_response(state).await,
        Request::ReleaseNotes => crate::about::release_notes(state).await,
        Request::Contributors => crate::about::contributors(state).await,
        Request::UninstallTool { tool } => uninstall_tool(&tool, state).await,
        Request::CheckUpdate { channel } => {
            let dl = crate::network::downloader(state).await;
            crate::self_update::check_update(channel, state, &dl, yerd_update::UPDATE_PUBLIC_KEY)
                .await
        }
        Request::CachedUpdateStatus => crate::self_update::cached_update_status(state).await,
        Request::SetUpdateChannel { channel } => {
            crate::self_update::set_update_channel(channel, state).await
        }
        Request::StageUpdate { channel } => {
            let dl = crate::network::downloader(state).await;
            crate::self_update::stage_update(channel, state, &dl, yerd_update::UPDATE_PUBLIC_KEY)
                .await
        }
        Request::StopTunnel { site } => crate::tunnel::stop_tunnel(&site, state).await,
        Request::TunnelStatus => crate::tunnel::tunnel_status(state).await,
        Request::CreateNamedTunnel { name } => crate::tunnel::named::create(&name, state).await,
        Request::ListNamedTunnels => crate::tunnel::named::list(state).await,
        Request::RouteTunnelDns { tunnel, hostname } => {
            crate::tunnel::named::route_dns(&tunnel, &hostname, state).await
        }
        Request::StopNamedTunnel => crate::tunnel::named::stop(state).await,
        Request::DeleteNamedTunnel { name } => crate::tunnel::named::delete(&name, state).await,
        Request::SetTunnelPassword { site, password } => {
            crate::tunnel::named::set_password(&site, password.as_deref(), state).await
        }
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

/// Each version paired with its CLI binary, the input to FFI probes.
fn cli_bins(
    state: &DaemonState,
    versions: &[yerd_core::PhpVersion],
) -> Vec<(yerd_core::PhpVersion, std::path::PathBuf)> {
    versions
        .iter()
        .map(|v| (*v, crate::php_install::cli_binary_path(&state.dirs, *v)))
        .collect()
}

/// The per-version ini overrides to hand the live `PhpManager`: the config's
/// overrides, FFI-gated so a build without FFI never gets an `ffi.enable` pool
/// line (see [`crate::ffi_cache::FfiCache::gate_ini_overrides`]). Every path
/// that pushes settings into the manager goes through this.
pub(crate) async fn manager_ini_overrides(
    state: &DaemonState,
    global: &std::collections::BTreeMap<String, String>,
    overrides: &std::collections::BTreeMap<
        yerd_core::PhpVersion,
        std::collections::BTreeMap<String, String>,
    >,
) -> std::collections::BTreeMap<yerd_core::PhpVersion, std::collections::BTreeMap<String, String>> {
    let bins = cli_bins(state, &installed_versions(state));
    state
        .ffi_cache
        .gate_ini_overrides(global, overrides, &bins)
        .await
}

/// Build the `PhpVersions` reply: installed versions, the live global default,
/// cached update annotations, the global ini settings, which versions include
/// FFI (cached probe), whether libvips was found, and the per-version Xdebug
/// settings plus which installed versions can use it. Read-only; no network.
pub(crate) async fn php_versions_response(state: &DaemonState) -> Response {
    let installed = installed_versions(state);
    let bins = cli_bins(state, &installed);
    let ffi_versions = state.ffi_cache.capable_versions(&bins).await;
    let (os, arch) = crate::ext_install::host_os_arch();
    let xdebug_versions: Vec<yerd_core::PhpVersion> = installed
        .iter()
        .copied()
        .filter(|v| yerd_core::php_xdebug::is_available(*v, os, arch))
        .collect();
    let (default, settings, version_settings, directives, pool, xdebug) = {
        let cfg = state.config.lock().await;
        (
            cfg.php.default,
            cfg.php.settings.clone(),
            cfg.php.version_settings.clone(),
            cfg.php.directives.clone(),
            cfg.php.pool.clone(),
            cfg.php.xdebug.clone(),
        )
    };
    Response::PhpVersions {
        installed,
        default,
        updates: Box::new(crate::php_updates::cached_updates(state).await),
        settings,
        version_settings: Box::new(version_settings),
        directives: Box::new(directives),
        pool: Box::new(pool),
        ffi_versions: Box::new(ffi_versions),
        libvips_found: state.libvips_probe.libvips_found(),
        xdebug: Box::new(xdebug),
        xdebug_versions: Box::new(xdebug_versions),
    }
}

/// `available php` - list the major.minor versions installable from the signed
/// `php.json` manifest, plus what's already installed (so clients hide or tag
/// them). Fetches + verifies the manifest on demand; a fetch/transport OR
/// signature-verification failure is an error (an empty parse result is still a
/// valid empty list).
async fn available_php_response(state: &DaemonState) -> Response {
    let dl = crate::network::downloader(state).await;
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
    let listing =
        match crate::php_install::fetch_verified_listing(dl, public_key, yerd_php::Channel::Stable)
            .await
        {
            Ok(body) => body,
            Err(e) => return internal(format!("couldn't load the PHP listing: {e}")),
        };
    let legacy =
        crate::php_install::fetch_verified_listing(dl, public_key, yerd_php::Channel::Legacy)
            .await
            .ok()
            .map(|body| yerd_php::available_minors(&body, os, arch, yerd_php::Channel::Legacy))
            .unwrap_or_default();
    Response::AvailablePhp {
        available: yerd_php::available_minors(&listing, os, arch, yerd_php::Channel::Stable),
        installed: installed_versions(state),
        legacy,
    }
}

/// Whether a dev tool is installed but Yerd's `{data}/bin` isn't on the user's
/// PATH yet - drives the doctor's [`yerd_ipc::DiagnosisCode::BinDirNotOnPath`]
/// warning. Checks the login shell's real PATH (via the cached
/// [`crate::tools::external::resolve_user_path`]) and the user's rc files; see
/// [`yerd_platform::pure::shell_profile::bin_dir_needs_setup`]. `Some(false)`
/// when no tool is installed or PATH is already wired; `None` when
/// undeterminable (non-Unix, or `$HOME` unset). Computed on demand from the
/// `Diagnose` handler, not on the per-poll status path. The cover/pcov shims
/// alone don't count - the gate is an actual installed dev tool.
async fn path_needs_setup(state: &DaemonState) -> Option<bool> {
    #[cfg(not(unix))]
    {
        let _ = state;
        None
    }
    #[cfg(unix)]
    {
        use yerd_platform::pure::shell_profile::{self, rc_relpaths, HostOs, Shell};

        let node_default = state.config.lock().await.node.default.clone();
        let any_tool = crate::tools::list_status(&state.dirs, node_default.as_deref())
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
        let rc_contents: Vec<String> = [Shell::Zsh, Shell::Bash, Shell::Fish, Shell::Posix]
            .into_iter()
            .flat_map(|s| rc_relpaths(s, os))
            .filter_map(|rel| std::fs::read_to_string(home.join(rel)).ok())
            .collect();
        let login_path = crate::tools::external::resolve_user_path().await;
        Some(shell_profile::bin_dir_needs_setup(
            login_path.as_deref(),
            &crate::tools::bin_dir(&state.dirs),
            &home,
            &rc_contents,
        ))
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

    let (
        sites,
        tld,
        default_php,
        mail_enabled,
        mail_port,
        symlink_protection,
        mcp_enabled,
        lan_enabled,
        shadows,
    ) = {
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
            cfg.mcp_enabled,
            cfg.lan_enabled,
            shadows,
        )
    };

    // LAN effective signals: discover the IP (gated on LAN being on) and read
    // whether the bootstrap listener actually bound. `None` when LAN is off.
    let (lan_ip, lan_setup_bound) = if lan_enabled {
        let bound = state
            .lan_setup_bound
            .load(std::sync::atomic::Ordering::Relaxed);
        (state.lan_ip, Some(bound))
    } else {
        (None, None)
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

    let browser_fp = state.ca_fingerprint;
    let browser_trust = tokio::task::spawn_blocking(move || {
        use yerd_platform::TrustStore;
        yerd_platform::ActiveTrustStore::new()
            .browser_ca_trust(&browser_fp)
            .ok()
            .map(map_browser_trust)
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

    let (
        port_redirect,
        foreign_web_listener,
        port_redirect_targets,
        lan_redirect_targets,
        legacy_pf_redirect,
    ) = tokio::task::spawn_blocking(|| {
        use yerd_platform::PortRedirector;
        let r = yerd_platform::ActivePortRedirector::new();
        (
            r.is_active(),
            r.foreign_web_listener(),
            r.redirect_targets().map(redirect_targets_to_wire),
            r.lan_redirect_targets().map(redirect_targets_to_wire),
            r.legacy_pf_redirect(),
        )
    })
    .await
    .unwrap_or((None, None, None, None, None));

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
            browser_trust,
        },
        resolver_installed,
        port_redirect,
        foreign_web_listener,
        port_redirect_targets,
        lan_redirect_targets,
        legacy_pf_redirect,
        node_updates: state
            .node_update_count
            .load(std::sync::atomic::Ordering::Relaxed),
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
        mcp_enabled,
        lan_enabled,
        lan_ip,
        lan_setup_bound,
    }
}

/// Map the platform layer's parsed anchor targets `(http, https)` to the wire
/// type.
fn redirect_targets_to_wire((http, https): (u16, u16)) -> yerd_ipc::PortRedirectTargets {
    yerd_ipc::PortRedirectTargets { http, https }
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
    let outbound = crate::network::doctor_probe(state).await;
    let manual = yerd_doctor::diagnose(
        &after,
        path_needs_setup(state).await,
        &crate::services::local_override_files(&state.dirs),
        Some(&outbound),
    )
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
#[allow(clippy::too_many_lines)]
async fn update_php(version: Option<yerd_core::PhpVersion>, state: &DaemonState) -> Response {
    let dl = crate::network::downloader(state).await;
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
    let key = yerd_update::PHP_LISTING_PUBLIC_KEY;
    let has_stable = targets.iter().any(|v| !v.is_legacy());
    let has_legacy = targets.iter().any(|v| v.is_legacy());
    let stable = if has_stable {
        match crate::php_install::fetch_verified_listing(&dl, key, yerd_php::Channel::Stable).await
        {
            Ok(body) => Some(body),
            Err(e) => return internal(format!("listing fetch/verify failed: {e}")),
        }
    } else {
        None
    };
    let legacy = if has_legacy {
        crate::php_install::fetch_verified_listing(&dl, key, yerd_php::Channel::Legacy)
            .await
            .ok()
    } else {
        None
    };
    let _guard = state.php_mutate.lock().await;
    let mut updated: Vec<yerd_core::PhpVersion> = Vec::new();
    let mut pending_error: Option<yerd_php::PhpError> = None;
    for minor in targets {
        let Some(installed) = crate::php_install::installed_patch(&state.dirs, minor) else {
            continue;
        };
        let installed_rev = crate::php_install::installed_revision(&state.dirs, minor);
        let channel = yerd_php::Channel::of(minor);
        let listing = match channel {
            yerd_php::Channel::Stable => stable.as_deref().unwrap_or_default(),
            yerd_php::Channel::Legacy => match legacy.as_deref() {
                Some(body) => body,
                None if version.is_some() => {
                    return internal(
                        "couldn't load the legacy PHP listing; try again later".to_owned(),
                    )
                }
                None => continue,
            },
        };
        let artifact = match yerd_php::resolve_from_listing(listing, minor, os, arch, channel) {
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
/// update itself already succeeded. The FFI-gated overrides are re-pushed first,
/// since an updated build may have gained or lost FFI. Runs under the caller's
/// `php_mutate` guard and takes `php_settings_mutate` itself, so a concurrent
/// settings save cannot be overwritten by this stale snapshot.
async fn restart_updated_pools(state: &DaemonState, updated: &[yerd_core::PhpVersion]) {
    if updated.is_empty() {
        return;
    }
    let _settings_guard = state.php_settings_mutate.lock().await;
    let (global, overrides) = {
        let cfg = state.config.lock().await;
        (cfg.php.settings.clone(), cfg.php.version_settings.clone())
    };
    let gated = manager_ini_overrides(state, &global, &overrides).await;
    let mut mgr = state.php_manager.lock().await;
    mgr.set_ini_overrides(gated);
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
/// Reject an install of an out-of-support legacy minor (< 8.2) that did not
/// carry the explicit `confirm_legacy` opt-in. Defence-in-depth behind the CLI
/// `--legacy` flag / GUI confirmation checkbox: the daemon never installs a
/// legacy version "by accident". `None` means the install may proceed.
fn legacy_install_gate(version: yerd_core::PhpVersion, confirm_legacy: bool) -> Option<Response> {
    if version.is_legacy() && !confirm_legacy {
        return Some(Response::Error {
            code: ErrorCode::LegacyRestricted,
            message: format!(
                "PHP {version} is an out-of-support legacy version. Re-run with explicit \
                 confirmation (CLI `--legacy`, or tick the confirmation box in the GUI). \
                 Legacy versions get no security support, no code coverage (phpcover), and \
                 no yerd-dumps, and cannot be set as the default."
            ),
        });
    }
    None
}

async fn install_php(
    version: yerd_core::PhpVersion,
    confirm_legacy: bool,
    state: &DaemonState,
) -> Response {
    if let Some(rejection) = legacy_install_gate(version, confirm_legacy) {
        return rejection;
    }
    let dl = crate::network::downloader(state).await;
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
    confirm_legacy: bool,
    state: Arc<DaemonState>,
) -> Response {
    if let Some(rejection) = legacy_install_gate(version, confirm_legacy) {
        return rejection;
    }
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
        let dl = crate::network::downloader(&state).await;
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
    if version.is_legacy() {
        return;
    }
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
/// the live `PhpManager`. Mirrors the discovery done at startup. Holds
/// `php_settings_mutate` while it re-pushes the FFI-gated overrides, so a
/// concurrent settings save cannot be overwritten by a stale snapshot.
async fn refresh_php_binaries(state: &DaemonState) {
    let binaries: std::collections::BTreeMap<yerd_core::PhpVersion, std::path::PathBuf> =
        match yerd_php::discover_bundled(&state.dirs) {
            Ok(b) => b.into_iter().collect(),
            Err(e) => {
                tracing::warn!(error = %e, "bundled PHP re-discovery failed after install");
                return;
            }
        };
    let _settings_guard = state.php_settings_mutate.lock().await;
    let (global, overrides) = {
        let cfg = state.config.lock().await;
        (cfg.php.settings.clone(), cfg.php.version_settings.clone())
    };
    let gated = manager_ini_overrides(state, &global, &overrides).await;
    let mut mgr = state.php_manager.lock().await;
    mgr.set_binaries(binaries);
    mgr.set_ini_overrides(gated);
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

/// Map the platform browser-trust probe to the wire enum.
fn map_browser_trust(t: yerd_platform::BrowserCaTrust) -> yerd_ipc::BrowserTrust {
    match t {
        yerd_platform::BrowserCaTrust::Trusted => yerd_ipc::BrowserTrust::Trusted,
        yerd_platform::BrowserCaTrust::Untrusted => yerd_ipc::BrowserTrust::Untrusted,
        yerd_platform::BrowserCaTrust::ToolMissing => yerd_ipc::BrowserTrust::ToolMissing,
    }
}

/// Install (or remove) the Yerd CA into the per-user **browser** NSS stores.
/// Runs unprivileged as the daemon's user; the CA PEM is read from the daemon's
/// on-disk `ca_path`, so no cert material crosses the IPC boundary.
async fn trust_browsers(uninstall: bool, state: &DaemonState) -> Response {
    let ca_path = state.ca_path.clone();
    let joined = tokio::task::spawn_blocking(move || {
        use yerd_platform::TrustStore;
        let ts = yerd_platform::ActiveTrustStore::new();
        if uninstall {
            ts.uninstall_firefox_nss()
        } else {
            ts.install_firefox_nss(&ca_path)
        }
    })
    .await;

    match joined {
        Ok(Ok(o)) => Response::BrowserTrust {
            attempted: o.profiles_attempted,
            succeeded: o.profiles_succeeded,
            certutil_missing: o.certutil_missing,
        },
        Ok(Err(e)) => Response::Error {
            code: yerd_ipc::ErrorCode::Internal,
            message: format!("browser NSS trust failed: {e}"),
        },
        Err(e) => Response::Error {
            code: yerd_ipc::ErrorCode::Internal,
            message: format!("browser NSS trust task panicked: {e}"),
        },
    }
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
    let (settings, extensions, version_settings, directives) = {
        let cfg = state.config.lock().await;
        (
            cfg.php.settings.clone(),
            cfg.php.extensions.clone(),
            cfg.php.version_settings.clone(),
            cfg.php.directives.clone(),
        )
    };
    if let Err(e) = crate::php_install::write_cli_ini(
        &state.dirs,
        &settings,
        state.php_ca_bundle.as_deref(),
        &extensions,
        &version_settings,
        &directives,
    ) {
        tracing::warn!(error = %e, "failed to write CLI php.ini");
    }
}

/// Reconcile the managed PHP shims against the installed set.
async fn reconcile_shims_now(state: &DaemonState) {
    reconcile_shims_for(state).await;
}

/// Reconcile the dev-tool shims (`composer`/`node`/`npm`/`npx`/`bun`/`bunx`/`pnpm`/`pnpx`) under
/// the **shared** `shim_reconcile` mutex (same dir as the PHP reconcile).
/// Best-effort: failures are logged. Used at startup and after install/uninstall,
/// so it also recounts pending Node updates against the installed set.
pub(crate) async fn reconcile_tool_shims_now(state: &DaemonState) {
    crate::node_updates::recount(state).await;
    let Some(yerd_bin) = yerd_sibling() else {
        tracing::warn!("cannot locate the `yerd` binary; skipping tool-shim reconcile");
        return;
    };
    let node_default = state.config.lock().await.node.default.clone();
    let _guard = state.shim_reconcile.lock().await;
    if let Err(e) =
        crate::tools::reconcile_tool_shims(&state.dirs, &yerd_bin, node_default.as_deref())
    {
        tracing::warn!(error = %e, "tool-shim reconcile failed");
    }
}

/// Reconcile the service client shims (`mysql`, `psql`, `valkey-cli`, ...) and
/// the `cloudflared` link under the **shared** `shim_reconcile` mutex.
/// Best-effort: failures are logged. The `cloudflared` link needs no `yerd`
/// binary, so it is reconciled even when the sibling cannot be located. Used at
/// startup, after every service install/uninstall/version change, and after a
/// `cloudflared` install.
pub(crate) async fn reconcile_client_shims_now(state: &DaemonState) {
    let _guard = state.shim_reconcile.lock().await;
    if let Err(e) = crate::tunnel::install::reconcile_cloudflared_shim(&state.dirs) {
        tracing::warn!(error = %e, "cloudflared shim reconcile failed");
    }
    let Some(yerd_bin) = yerd_sibling() else {
        tracing::warn!("cannot locate the `yerd` binary; skipping service-client shim reconcile");
        return;
    };
    if let Err(e) = crate::service_install::reconcile_client_shims(&state.dirs, &yerd_bin) {
        tracing::warn!(error = %e, "service-client shim reconcile failed");
    }
}

/// Build the tool list and tag any *not* Yerd-managed tool that's available on
/// the user's PATH as `external` (Tooling shows an "External" badge but still
/// offers **Install**, to add yerd's own copy alongside it). Tools that
/// [don't accept an external copy](crate::tools::Tool::accepts_external) are
/// never tagged - yerd needs its own build, so Tooling shows them as simply not
/// installed. Skips the (login-shell) PATH resolution entirely when no remaining
/// tool could be tagged.
async fn list_tools_with_external(state: &DaemonState) -> Vec<yerd_ipc::ToolStatus> {
    let node_default = state.config.lock().await.node.default.clone();
    let mut tools = crate::tools::list_status(&state.dirs, node_default.as_deref());
    crate::tool_updates::fill_update_available(state, &mut tools).await;
    let taggable = |t: &yerd_ipc::ToolStatus| {
        !t.installed
            && crate::tools::Tool::parse(&t.id).is_some_and(crate::tools::Tool::accepts_external)
    };
    if !tools.iter().any(taggable) {
        return tools;
    }
    let Some(dirs) = crate::tools::external::resolve_user_path().await else {
        return tools;
    };
    let data_bin = crate::tools::bin_dir(&state.dirs);
    let data_root = &state.dirs.data;
    for t in &mut tools {
        if !taggable(t) {
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
    let dl = crate::network::downloader(state).await;
    let _mutate = state.tool_mutate.lock().await;
    install_tool_with(t, state, &dl).await
}

/// Injectable core of [`install_tool`]. The caller holds `tool_mutate`.
async fn install_tool_with(
    t: crate::tools::Tool,
    state: &DaemonState,
    dl: &dyn yerd_php::Downloader,
) -> Response {
    let previous = node_default_before_install(t, state).await;
    let proxy_env = crate::network::child_env_plan(state).await;
    match crate::tools::install(t, &state.dirs, dl, &proxy_env, None).await {
        Ok(()) => finish_tool_install(t, state, dl, previous).await,
        Err(e) => Response::Error {
            code: tool_error_code(&e),
            message: e.to_string(),
        },
    }
}

/// The effective global Node default before a no-version Node install, which
/// rewrites the legacy pointer. `None` for other tools.
async fn node_default_before_install(t: crate::tools::Tool, state: &DaemonState) -> Option<String> {
    if t != crate::tools::Tool::Node {
        return None;
    }
    effective_node_default(state).await
}

/// Follow-up after a successful tool install, shared by the blocking and
/// streamed paths. For Node, a configured default is kept; otherwise the
/// previous effective default (a legacy pointer) is persisted when it still
/// resolves, and otherwise the newly installed release becomes it.
async fn finish_tool_install(
    t: crate::tools::Tool,
    state: &DaemonState,
    dl: &dyn yerd_php::Downloader,
    previous: Option<String>,
) -> Response {
    if t == crate::tools::Tool::Node {
        crate::node_updates::poll_and_refresh(state, dl).await;
        let configured = state.config.lock().await.node.default.is_some();
        if !configured {
            if let Some(version) = previous
                .filter(|v| yerd_platform::node::resolve(&state.dirs, v).is_ok())
                .or_else(|| yerd_platform::node::legacy_default(&state.dirs))
            {
                let result = save_node_version(&version, None, state).await;
                if !matches!(result, Response::Ok) {
                    return result;
                }
            }
        }
    }
    reconcile_tool_shims_now(state).await;
    Response::Ok
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
        let dl = crate::network::downloader(&state).await;
        let _guard = state.tool_mutate.lock().await;
        let previous = node_default_before_install(t, &state).await;
        let proxy_env = crate::network::child_env_plan(&state).await;
        let result = tokio::select! {
            r = crate::tools::install(t, &state.dirs, &dl, &proxy_env, Some(&tx)) => Some(r),
            _ = cancel.changed() => None,
        };
        drop(tx);
        let _ = drain.await;

        match result {
            Some(Ok(())) => {
                if let Response::Error { message, .. } =
                    finish_tool_install(t, &state, &dl, previous).await
                {
                    state
                        .jobs
                        .finish(&id, yerd_ipc::JobState::Failed, Some(message))
                        .await;
                    return;
                }
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
            if t == crate::tools::Tool::Node {
                let mut guard = state.config.lock().await;
                let mut new = guard.clone();
                new.node.default = None;
                new.node.sites.clear();
                if let Err(e) = new.save(&state.config_path) {
                    return internal(format!("config save failed: {e}"));
                }
                *guard = new;
            }
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
    let dl = crate::network::downloader(state).await;
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
///
/// The version's `[php.xdebug]` entry is dropped and saved before success is
/// reported. If that save or removing its Xdebug files fails the error is
/// returned; a retry finds the version already gone and only finishes the
/// Xdebug cleanup, recognised by the config entry or the files left behind.
/// That retry answers with the plain PHP list like a real uninstall, so the
/// CLI and GUI need no new response shape, and its errors never claim the
/// version was removed by this request.
async fn uninstall_php(version: yerd_core::PhpVersion, state: &DaemonState) -> Response {
    let installed = installed_versions(state);
    if !installed.contains(&version) {
        let has_entry = state.config.lock().await.php.xdebug.contains_key(&version);
        let leftovers = if has_entry {
            Ok(true)
        } else {
            xdebug_files_left(&state.dirs, version)
        };
        return match leftovers {
            Err(e) => internal(format!(
                "couldn't check for leftover Xdebug files for PHP {version}: {e}; fix and retry \
                 `yerd uninstall php {version}`"
            )),
            Ok(true) => match remove_xdebug_leftovers(state, version).await {
                Ok(()) => {
                    tracing::info!(
                        version = %version,
                        "PHP {version} isn't installed; removed leftover Xdebug files"
                    );
                    php_versions_response(state).await
                }
                Err(e) => internal(format!(
                    "couldn't remove leftover Xdebug files for PHP {version}: {e}; fix and \
                     retry `yerd uninstall php {version}`"
                )),
            },
            Ok(false) => Response::Error {
                code: ErrorCode::NotFound,
                message: format!("PHP {version} is not installed"),
            },
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
    let cleanup = remove_xdebug_leftovers(state, version).await;
    refresh_php_binaries(state).await;
    reconcile_shims_now(state).await;
    if let Err(e) = cleanup {
        return internal(format!(
            "PHP {version} was removed, but its Xdebug cleanup failed: {e}. Fix that and run \
             `yerd uninstall php {version}` to finish"
        ));
    }
    tracing::info!(version = %version, "uninstalled PHP");
    php_versions_response(state).await
}

/// An uninstalled version's bundled `xdebug.so` and `debug.ini`.
fn xdebug_files(
    dirs: &yerd_platform::PlatformDirs,
    version: yerd_core::PhpVersion,
) -> [std::path::PathBuf; 2] {
    [
        crate::ext_install::xdebug_so_path(dirs, version),
        crate::xdebug::debug_ini_path(dirs, version),
    ]
}

/// True if any of `version`'s [`xdebug_files`] is still on disk. Only
/// `NotFound` counts as absent.
///
/// # Errors
/// A human message naming the path when one can't be inspected for any other
/// reason, so the caller never reports a clean uninstall it couldn't verify.
fn xdebug_files_left(
    dirs: &yerd_platform::PlatformDirs,
    version: yerd_core::PhpVersion,
) -> Result<bool, String> {
    let mut left = false;
    for path in xdebug_files(dirs, version) {
        match std::fs::symlink_metadata(&path) {
            Ok(_) => left = true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("couldn't inspect {} ({e})", path.display())),
        }
    }
    Ok(left)
}

/// Remove an uninstalled version's [`xdebug_files`], then drop its
/// `[php.xdebug]` entry. A file that is already gone counts as removed.
///
/// # Errors
/// The failures joined into one human message when a file can't be removed or
/// the config save fails. The caller frames it and names the retry.
async fn remove_xdebug_leftovers(
    state: &DaemonState,
    version: yerd_core::PhpVersion,
) -> Result<(), String> {
    let mut failed: Vec<String> = xdebug_files(&state.dirs, version)
        .iter()
        .filter_map(|path| match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Some(format!("couldn't remove {} ({e})", path.display()))
            }
            _ => None,
        })
        .collect();
    if let Err(e) = forget_xdebug(state, version).await {
        failed.push(e);
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}

/// Drop an uninstalled version's `[php.xdebug]` entry (saving only when one
/// existed) and push the reduced load map into the manager.
///
/// # Errors
/// A human message when the config save fails; the entry is then kept.
async fn forget_xdebug(state: &DaemonState, version: yerd_core::PhpVersion) -> Result<(), String> {
    let _mutate_guard = state.php_settings_mutate.lock().await;
    let (map, saved) = {
        let mut cfg_guard = state.config.lock().await;
        let mut saved = Ok(());
        if cfg_guard.php.xdebug.contains_key(&version) {
            let mut new = cfg_guard.clone();
            new.php.xdebug.remove(&version);
            match new.save(&state.config_path) {
                Ok(()) => *cfg_guard = new,
                Err(e) => {
                    saved = Err(format!("dropping its Xdebug settings failed ({e})"));
                }
            }
        }
        (
            crate::xdebug::xdebug_load_map(&cfg_guard, &state.dirs),
            saved,
        )
    };
    state.php_manager.lock().await.set_xdebug(map);
    saved
}

/// `use <ver>` (global) - require the version installed, set the live default +
/// site fallback (`config.php.default`), persist, and repoint the `php` shim.
async fn set_default_php(version: yerd_core::PhpVersion, state: &DaemonState) -> Response {
    if version.is_legacy() {
        return Response::Error {
            code: ErrorCode::LegacyRestricted,
            message: format!(
                "PHP {version} is an out-of-support legacy version and cannot be the global \
                 default. It can still serve individual sites and run via `php{version}`."
            ),
        };
    }
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

/// Enable or disable the MCP server gate. Persist-only: the daemon runs no MCP
/// server, so there is nothing live to update. Each `yerd mcp` session reads the
/// flag back from [`Request::Status`], so enabling reaches running agent
/// sessions on their next tool call.
async fn set_mcp_enabled(enabled: bool, state: &DaemonState) -> Response {
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    new.mcp_enabled = enabled;
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *cfg_guard = new;
    tracing::info!(enabled, "set mcp enabled");
    Response::Ok
}

/// Persist the `lan_enabled` flag. Persist-only: the bootstrap listener only
/// re-binds on the daemon restart the CLI triggers next (a listen socket's bind
/// address is fixed at bind time), so there is no live atomic to flip here.
///
/// Disabling also revokes any pending one-time remote-setup code, since the
/// listener lingers until that restart and an already-minted code would
/// otherwise stay redeemable. The revocation happens while the `config` lock is
/// held so it and [`mint_remote_setup_code`]'s check-and-publish cannot
/// interleave - otherwise a mint that had already passed its `lan_enabled` check
/// could store a live code *after* disablement completed.
async fn set_lan_enabled(enabled: bool, state: &DaemonState) -> Response {
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    new.lan_enabled = enabled;
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *cfg_guard = new;
    if !enabled {
        *state.remote_setup_code.lock().await = None;
    }
    drop(cfg_guard);
    tracing::info!(enabled, "set lan enabled");
    Response::Ok
}

/// TTL for a minted remote-setup code.
const REMOTE_SETUP_CODE_TTL: std::time::Duration = std::time::Duration::from_secs(900);

/// Mint a one-time bootstrap code. Guarded on LAN actually being up (config on
/// *and* the bootstrap listener bound), so a code is never handed out when
/// nothing would serve it.
///
/// The `config` lock is held across the whole check-and-publish so a concurrent
/// [`set_lan_enabled`]`(false)` cannot slip between the `lan_enabled` check and
/// storing the code (which would leave a live code after disablement). The URL
/// reuses the startup-discovered `lan_ip` so the printed host always matches what
/// is actually served, and the returned `script_sha256` is the hash recorded when
/// the endpoint bound - its absence means the endpoint isn't serving, so minting
/// fails closed rather than hand out a useless code.
async fn mint_remote_setup_code(state: &DaemonState) -> Response {
    let cfg_guard = state.config.lock().await;
    if !cfg_guard.lan_enabled {
        return lan_not_ready("LAN mode is off - run `yerd lan enable` first".to_owned());
    }
    let lan_setup_port = cfg_guard.lan_setup_port;
    if !state
        .lan_setup_bound
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return lan_not_ready(
            "the remote-setup listener isn't bound (restart the daemon after enabling LAN)"
                .to_owned(),
        );
    }
    let Some(lan_ip) = state.lan_ip else {
        return lan_not_ready("the LAN IP isn't known (restart the daemon)".to_owned());
    };

    let Some(script_sha256) = state.lan_setup_script_sha256.lock().await.clone() else {
        return lan_not_ready(
            "the installer hash isn't ready (restart the daemon after enabling LAN)".to_owned(),
        );
    };

    let mut bytes = [0u8; 16];
    {
        use rand::RngCore as _;
        rand::thread_rng().fill_bytes(&mut bytes);
    }
    let code = hex::encode(bytes);

    *state.remote_setup_code.lock().await = Some(crate::state::RemoteSetupCode {
        value: code.clone(),
        expires_at: std::time::Instant::now() + REMOTE_SETUP_CODE_TTL,
        used: false,
    });
    drop(cfg_guard);

    let url = format!("http://{lan_ip}:{lan_setup_port}/remote-setup?code={code}");
    Response::RemoteSetup {
        code,
        url,
        script_sha256,
        expires_in_secs: REMOTE_SETUP_CODE_TTL.as_secs(),
    }
}

/// `set/unset php` - merge global PHP ini settings into the config and apply
/// them to every live FPM pool. An empty-string value removes a key (reset to
/// PHP's default).
///
/// Order (build → validate → save → commit → drop config guard → restart
/// pools): the config write is fail-closed; the per-pool restart is best-effort
/// and runs *after* the config guard is released, under a single `php_manager`
/// lock so no request can `ensure` a stale-config pool mid-update. The whole
/// sequence holds `php_settings_mutate`, so an overlapping settings request
/// can't push a stale snapshot into the manager after a newer one applied.
async fn set_php_settings(
    settings: std::collections::BTreeMap<String, String>,
    state: &DaemonState,
) -> Response {
    let _mutate_guard = state.php_settings_mutate.lock().await;
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
    let applied = new.php.settings.clone();
    let overrides = new.php.version_settings.clone();
    *cfg_guard = new;
    drop(cfg_guard);
    let gated = manager_ini_overrides(state, &applied, &overrides).await;

    {
        let mut mgr = state.php_manager.lock().await;
        mgr.set_ini_settings(applied);
        mgr.set_ini_overrides(gated);
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

/// `set/unset php --only <version>` - merge per-version overrides of the
/// allowlisted settings into the config and apply them to that version's FPM
/// pool and CLI ini. An empty-string value removes the override (the global
/// value applies again). Same lock order and `php_settings_mutate` discipline
/// as [`set_php_settings`], but only the affected version's pool restarts.
async fn set_php_version_settings(
    version: yerd_core::PhpVersion,
    settings: std::collections::BTreeMap<String, String>,
    state: &DaemonState,
) -> Response {
    if let Some(resp) = require_installed(version, state) {
        return resp;
    }
    let _mutate_guard = state.php_settings_mutate.lock().await;
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    for (key, value) in settings {
        if value.is_empty() {
            if let Some(map) = new.php.version_settings.get_mut(&version) {
                map.remove(&key);
            }
            continue;
        }
        if let Err(e) = yerd_core::php_settings::validate_version_value(&key, &value, version) {
            return Response::Error {
                code: ErrorCode::InvalidPath,
                message: e.to_string(),
            };
        }
        let canonical = yerd_core::php_settings::canonical_value(&key, &value);
        new.php
            .version_settings
            .entry(version)
            .or_default()
            .insert(key, canonical);
    }
    if new
        .php
        .version_settings
        .get(&version)
        .is_some_and(std::collections::BTreeMap::is_empty)
    {
        new.php.version_settings.remove(&version);
    }

    if new.php.version_settings == cfg_guard.php.version_settings {
        drop(cfg_guard);
        return php_versions_response(state).await;
    }

    if let Err(e) = new.validate() {
        return internal(format!("config validation failed: {e}"));
    }
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *cfg_guard = new;
    drop(cfg_guard);

    apply_version_php_config(state, version).await;
    tracing::info!(version = %version, "applied per-version PHP settings");
    php_versions_response(state).await
}

/// `yerd php ini set/unset` - merge free-form per-version ini directives into
/// the config and apply them to that version's FPM pool and CLI ini. An
/// empty-string value removes the directive. Reserved names (allowlisted
/// settings, extension loading, the CA bundle) are refused with a pointer to
/// the typed path. Same lock order and `php_settings_mutate` discipline as
/// [`set_php_settings`], but only the affected version's pool restarts.
async fn set_php_directives(
    version: yerd_core::PhpVersion,
    directives: std::collections::BTreeMap<String, String>,
    state: &DaemonState,
) -> Response {
    if let Some(resp) = require_installed(version, state) {
        return resp;
    }
    let _mutate_guard = state.php_settings_mutate.lock().await;
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    for (key, value) in directives {
        if let Err(e) = yerd_core::php_directives::validate_name(&key) {
            return Response::Error {
                code: ErrorCode::InvalidPath,
                message: e.to_string(),
            };
        }
        if let Some(hint) = yerd_core::php_directives::reserved(&key) {
            return Response::Error {
                code: ErrorCode::InvalidPath,
                message: format!("{key} is managed by Yerd: {hint}"),
            };
        }
        if value.is_empty() {
            if let Some(map) = new.php.directives.get_mut(&version) {
                map.remove(&key);
            }
            continue;
        }
        if let Err(e) = yerd_core::php_directives::validate_value(&value) {
            return Response::Error {
                code: ErrorCode::InvalidPath,
                message: e.to_string(),
            };
        }
        new.php
            .directives
            .entry(version)
            .or_default()
            .insert(key, value.trim().to_owned());
    }
    if new
        .php
        .directives
        .get(&version)
        .is_some_and(std::collections::BTreeMap::is_empty)
    {
        new.php.directives.remove(&version);
    }

    if new.php.directives == cfg_guard.php.directives {
        drop(cfg_guard);
        return php_versions_response(state).await;
    }

    if let Err(e) = new.validate() {
        return internal(format!("config validation failed: {e}"));
    }
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *cfg_guard = new;
    drop(cfg_guard);

    apply_version_php_config(state, version).await;
    tracing::info!(version = %version, "applied per-version PHP ini directives");
    php_versions_response(state).await
}

/// `yerd php pool set/unset` - merge per-version FPM pool settings into the
/// config and apply them to that version's pool. An empty-string value resets
/// the setting to its built-in default. Mirrors [`set_php_directives`]'s lock
/// order and `php_settings_mutate` discipline; only the affected version's
/// pool restarts. Values are stored canonically (`"032"` persists as `"32"`),
/// matching [`set_php_version_settings`].
async fn set_php_pool_settings(
    version: yerd_core::PhpVersion,
    settings: std::collections::BTreeMap<String, String>,
    state: &DaemonState,
) -> Response {
    if let Some(resp) = require_installed(version, state) {
        return resp;
    }
    let _mutate_guard = state.php_settings_mutate.lock().await;
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    for (key, value) in settings {
        if let Err(e) = yerd_core::php_pool::validate_name(&key) {
            return Response::Error {
                code: ErrorCode::InvalidPath,
                message: e.to_string(),
            };
        }
        if value.is_empty() {
            if let Some(map) = new.php.pool.get_mut(&version) {
                map.remove(&key);
            }
            continue;
        }
        let parsed = match yerd_core::php_pool::validate_value(&value) {
            Ok(n) => n,
            Err(e) => {
                return Response::Error {
                    code: ErrorCode::InvalidPath,
                    message: e.to_string(),
                }
            }
        };
        new.php
            .pool
            .entry(version)
            .or_default()
            .insert(key, parsed.to_string());
    }
    if new
        .php
        .pool
        .get(&version)
        .is_some_and(std::collections::BTreeMap::is_empty)
    {
        new.php.pool.remove(&version);
    }

    if new.php.pool == cfg_guard.php.pool {
        drop(cfg_guard);
        return php_versions_response(state).await;
    }

    if let Err(e) = new.validate() {
        return internal(format!("config validation failed: {e}"));
    }
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *cfg_guard = new;
    drop(cfg_guard);

    apply_version_php_config(state, version).await;
    tracing::info!(version = %version, "applied per-version FPM pool settings");
    php_versions_response(state).await
}

/// `NotFound` error when `version` has no installed CLI binary, else `None`.
/// Per-version config only makes sense for an installed version (mirrors
/// `add_php_extension`).
pub(crate) fn require_installed(
    version: yerd_core::PhpVersion,
    state: &DaemonState,
) -> Option<Response> {
    let php_bin = crate::php_install::cli_binary_path(&state.dirs, version);
    if php_bin.exists() {
        return None;
    }
    Some(Response::Error {
        code: ErrorCode::NotFound,
        message: format!("PHP {version} is not installed - run `yerd install php {version}`"),
    })
}

/// Push the config's per-version settings overrides, directives, and FPM pool
/// settings into the live `PhpManager`, restart the affected version's pool if
/// it is currently running, and rewrite the per-version CLI inis. Follows
/// `set_php_settings`'s lock discipline: the config lock is released before the
/// manager lock is taken. Runs under the caller's `php_settings_mutate` guard,
/// which is what keeps the config re-read here from racing a concurrent
/// settings mutation. The pool map never reaches `write_cli_ini`, so pool
/// settings cannot leak into a CLI `php.ini`.
async fn apply_version_php_config(state: &DaemonState, affected: yerd_core::PhpVersion) {
    let (global, version_settings, directives, pool) = {
        let cfg = state.config.lock().await;
        (
            cfg.php.settings.clone(),
            cfg.php.version_settings.clone(),
            cfg.php.directives.clone(),
            cfg.php.pool.clone(),
        )
    };
    let gated = manager_ini_overrides(state, &global, &version_settings).await;
    {
        let mut mgr = state.php_manager.lock().await;
        mgr.set_ini_overrides(gated);
        mgr.set_directives(directives);
        mgr.set_pool_overrides(pool);
        if mgr.snapshots().iter().any(|s| s.version == affected) {
            if let Err(e) = mgr.restart(affected).await {
                tracing::warn!(version = %affected, error = %e, "failed to restart FPM pool after per-version PHP config change");
            }
        }
    }
    write_cli_ini_now(state).await;
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

    {
        let cfg = state.config.lock().await;
        if let Some(resp) = managed_xdebug_conflict(&cfg, version, &name, &path) {
            return resp;
        }
        if cfg
            .php
            .extensions
            .get(&version)
            .is_some_and(|list| list.iter().any(|e| e.name == name))
        {
            return Response::Error {
                code: ErrorCode::AlreadyExists,
                message: format!(
                    "an extension named {name} is already registered for PHP {version}"
                ),
            };
        }
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

    let _mutate_guard = state.php_settings_mutate.lock().await;
    {
        let mut cfg_guard = state.config.lock().await;
        if let Some(resp) = managed_xdebug_conflict(&cfg_guard, version, &name, &path) {
            return resp;
        }
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

/// `AlreadyExists` when `name`/`path` is an Xdebug and Yerd's managed Xdebug is
/// on for `version`, so the same `.so` is never loaded twice.
fn managed_xdebug_conflict(
    cfg: &yerd_config::Config,
    version: yerd_core::PhpVersion,
    name: &str,
    path: &str,
) -> Option<Response> {
    let managed_on = cfg.php.xdebug.get(&version).is_some_and(|s| s.enabled);
    if !managed_on || !yerd_core::php_xdebug::is_xdebug_extension(name, path) {
        return None;
    }
    Some(Response::Error {
        code: ErrorCode::AlreadyExists,
        message: format!(
            "Xdebug is managed by Yerd for PHP {version}; use `yerd xdebug on/off` instead"
        ),
    })
}

/// Remove a registered extension by name for `version`. The commit and apply
/// run under `php_settings_mutate`, like the add path.
async fn remove_php_extension(
    version: yerd_core::PhpVersion,
    name: String,
    state: &DaemonState,
) -> Response {
    let _mutate_guard = state.php_settings_mutate.lock().await;
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
/// CLI inis. Runs under the caller's `php_settings_mutate` guard. Both load maps
/// come from one config snapshot (a hand-registered Xdebug suppresses the
/// managed one), and the config lock is released before the manager lock is
/// taken.
async fn apply_extensions(state: &DaemonState, affected: yerd_core::PhpVersion) {
    let (ext_map, xdebug_map) = {
        let cfg = state.config.lock().await;
        (
            extension_load_map(&cfg),
            crate::xdebug::xdebug_load_map(&cfg, &state.dirs),
        )
    };
    {
        let mut mgr = state.php_manager.lock().await;
        mgr.set_extensions(ext_map);
        mgr.set_xdebug(xdebug_map);
        if mgr.snapshots().iter().any(|s| s.version == affected) {
            if let Err(e) = mgr.restart(affected).await {
                tracing::warn!(version = %affected, error = %e, "failed to restart FPM pool after extension change");
            }
        }
    }
    write_cli_ini_now(state).await;
}

/// Build the `PhpManager`'s extension map (`version -> [ExtLoad]`) from `cfg`.
fn extension_load_map(
    cfg: &yerd_config::Config,
) -> std::collections::BTreeMap<yerd_core::PhpVersion, Vec<yerd_php::ExtLoad>> {
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

    let (candidate, candidate_wordpress, candidate_laravel, candidate_framework) =
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
    *state.framework_sites.write().await = candidate_framework;
    drop(cfg_guard);
    crate::tunnel::rebuild_routes(state).await;

    state.watch_dirty.notify_one();

    if let Some(site) = site_after {
        crate::wordpress_url_sync::sync_site_url(&site, state).await;
        crate::codeigniter_url_sync::sync_base_url(&site, state).await;
    }

    tracing::info!(summary = %applied.summary, "applied mutation");
    Response::Ok
}

/// The post-mutation site to run [`crate::wordpress_url_sync::sync_site_url`]
/// and [`crate::codeigniter_url_sync::sync_base_url`] against: `SetSecure`
/// (which flips the scheme) plus every domain mutation (each of which can
/// change the primary domain a WordPress install should advertise).
/// `AddDomain` is included, not just `SetPrimaryDomain`/`ResetDomains`
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
///
/// Domain fields are reported only for a proxy the router actually holds. A
/// name-shadowed proxy stays in the config but is never inserted, and the shared
/// domain maps are keyed by claimant name, so enriching it would report the
/// shadowing site's domains as the proxy's own.
async fn list_proxies(state: &DaemonState) -> Response {
    let cfg = state.config.lock().await;
    let router = state.router.read().await;
    let proxies = cfg
        .proxies
        .iter()
        .map(|p| {
            let (primary_domain, domains) = if router.proxy(p.name()).is_some() {
                site_entry_domains(&router, p.name(), cfg.tld.as_str())
            } else {
                (None, Vec::new())
            };
            yerd_ipc::ProxyEntry {
                name: p.name().to_owned(),
                target: p.target().to_string(),
                secure: p.secure(),
                primary_domain,
                domains,
            }
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

/// Reply to [`Request::ListRoutes`]: every per-site path-prefix routing rule.
/// Parked rules key by document-root, resolved through the live router to the
/// current site name exactly as [`list_proxies`] does, so the output round-trips
/// through `yerd route remove <site> <prefix>`. A parked docroot with no current
/// site falls back to the raw key.
async fn list_routes(state: &DaemonState) -> Response {
    let cfg = state.config.lock().await;
    let router = state.router.read().await;
    let mut rules = Vec::new();
    for (site, site_rules) in &cfg.route_rules.linked {
        for r in site_rules {
            rules.push(yerd_ipc::RouteRuleEntry {
                site: site.clone(),
                prefix: r.prefix().to_owned(),
                target: r.target().to_owned(),
            });
        }
    }
    for (docroot, site_rules) in &cfg.route_rules.parked {
        let site_name = router
            .iter()
            .find(|s| s.document_root().to_string_lossy().as_ref() == docroot.as_str())
            .map_or_else(|| docroot.clone(), |s| s.name().to_owned());
        for r in site_rules {
            rules.push(yerd_ipc::RouteRuleEntry {
                site: site_name.clone(),
                prefix: r.prefix().to_owned(),
                target: r.target().to_owned(),
            });
        }
    }
    Response::Routes { rules }
}

/// Reply to [`Request::ListResponseHeaders`]: every per-site custom response
/// header. Parked docroots resolve to the current site name exactly as
/// [`list_routes`] does, falling back to the raw key.
async fn list_response_headers(state: &DaemonState) -> Response {
    let cfg = state.config.lock().await;
    let router = state.router.read().await;
    let mut headers = Vec::new();
    for (site, site_headers) in &cfg.response_headers.linked {
        for h in site_headers {
            headers.push(yerd_ipc::ResponseHeaderEntry {
                site: site.clone(),
                name: h.name().to_owned(),
                value: h.value().to_owned(),
            });
        }
    }
    for (docroot, site_headers) in &cfg.response_headers.parked {
        let site_name = router
            .iter()
            .find(|s| {
                s.kind() == yerd_core::SiteKind::Parked
                    && s.document_root().to_string_lossy().as_ref() == docroot.as_str()
            })
            .map_or_else(|| docroot.clone(), |s| s.name().to_owned());
        for h in site_headers {
            headers.push(yerd_ipc::ResponseHeaderEntry {
                site: site_name.clone(),
                name: h.name().to_owned(),
                value: h.value().to_owned(),
            });
        }
    }
    Response::ResponseHeaders { headers }
}

/// Apply a group or favourite mutation (create/delete/reorder/assign, or
/// star/unstar). Both are config-only organisational overlays that never affect
/// routing, so this uses
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

    if matches!(req, Request::SetSiteFavourite { .. }) {
        *state.favourite_sites.write().await = new.favourites.sites.clone();
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
        message: mutate::not_found_site(new, &name_lc).to_string(),
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

fn lan_not_ready(message: String) -> Response {
    Response::Error {
        code: ErrorCode::LanNotReady,
        message,
    }
}

/// Install an explicit Node selector without replacing an existing default.
async fn install_node(version: &str, state: &DaemonState) -> Response {
    let _mutate = state.tool_mutate.lock().await;
    let had_default = node_had_default(state).await;
    let dl = crate::network::downloader(state).await;
    match crate::tools::node::install_release(&state.dirs, &dl, Some(version), None).await {
        Ok(installed) => {
            crate::node_updates::store(state, installed.latest).await;
            after_node_install(state, &installed.exact, had_default, false).await
        }
        Err(e) => Response::Error {
            code: tool_error_code(&e),
            message: e.to_string(),
        },
    }
}

/// Whether an effective global Node default exists (config value or legacy pointer).
async fn node_had_default(state: &DaemonState) -> bool {
    effective_node_default(state).await.is_some()
}

/// The effective global Node default: the config value, else a legacy pointer.
async fn effective_node_default(state: &DaemonState) -> Option<String> {
    let configured = state.config.lock().await.node.default.clone();
    yerd_platform::node::default_selector(&state.dirs, configured.as_deref())
}

/// `InstallNodeStreamed` - install a Node release as a background job with the
/// same job, drain, cancel and `tool_mutate` shape as [`install_tool_streamed`].
pub(crate) async fn install_node_streamed(
    version: String,
    update: bool,
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
            .set_phase(&id, format!("Installing Node {version}"))
            .await;
        let dl = crate::network::downloader(&state).await;
        let _guard = state.tool_mutate.lock().await;
        let had_default = node_had_default(&state).await;
        let result = tokio::select! {
            r = crate::tools::node::install_release(&state.dirs, &dl, Some(&version), Some(&tx)) => Some(r),
            _ = cancel.changed() => None,
        };
        drop(tx);
        let _ = drain.await;

        match result {
            Some(Ok(installed)) => {
                crate::node_updates::store(&state, installed.latest).await;
                match after_node_install(&state, &installed.exact, had_default, update).await {
                    Response::Error { message, .. } => {
                        state
                            .jobs
                            .finish(&id, yerd_ipc::JobState::Failed, Some(message))
                            .await;
                    }
                    _ => {
                        state
                            .jobs
                            .finish(&id, yerd_ipc::JobState::Succeeded, None)
                            .await;
                    }
                }
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

/// Config follow-up after a Node install, shared by the blocking and streamed
/// paths. The caller holds `tool_mutate`. The first install becomes the default;
/// an Update moves the effective default (config value or legacy pointer) to
/// `exact` only when it was on an older patch of that major. Site preferences
/// are never touched. The config guard is dropped before the shim reconcile,
/// which takes the config lock itself.
async fn after_node_install(
    state: &DaemonState,
    exact: &str,
    had_default: bool,
    update: bool,
) -> Response {
    let installed = yerd_platform::node::installed(&state.dirs);
    let pairs = node_pairs(&installed);
    {
        let mut guard = state.config.lock().await;
        let effective =
            yerd_platform::node::default_selector(&state.dirs, guard.node.default.as_deref());
        let next = if had_default {
            update
                .then(|| yerd_core::node::moved_default(effective.as_deref(), &pairs, exact))
                .flatten()
        } else {
            Some(exact.to_owned())
        };
        if let Some(next) = next {
            let mut new = guard.clone();
            new.node.default = Some(next);
            if let Err(e) = new.save(&state.config_path) {
                return internal(format!("config save failed: {e}"));
            }
            *guard = new;
        }
    }
    reconcile_tool_shims_now(state).await;
    Response::Ok
}

fn node_pairs(
    installed: &[yerd_platform::node::InstalledNode],
) -> Vec<yerd_core::node::Candidate<'_>> {
    installed
        .iter()
        .map(yerd_platform::node::InstalledNode::candidate)
        .collect()
}

async fn set_node_version(version: &str, site: Option<&str>, state: &DaemonState) -> Response {
    let _mutate = state.tool_mutate.lock().await;
    let result = save_node_version(version, site, state).await;
    if matches!(result, Response::Ok) {
        reconcile_tool_shims_now(state).await;
    }
    result
}

pub(crate) async fn save_node_version(
    version: &str,
    site: Option<&str>,
    state: &DaemonState,
) -> Response {
    let selected = match yerd_platform::node::resolve(&state.dirs, version) {
        Ok(v) => v,
        Err(message) => {
            return Response::Error {
                code: ErrorCode::NotFound,
                message,
            }
        }
    };
    let mut guard = state.config.lock().await;
    let mut new = guard.clone();
    if let Some(name) = site {
        let root = match site_project_root(state, name).await {
            Ok(root) => root,
            Err(response) => return response,
        };
        new.node
            .sites
            .insert(root.to_string_lossy().into_owned(), selected.version);
    } else {
        new.node.default = Some(selected.version);
    }
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *guard = new;
    Response::Ok
}

/// The canonical project root that keys a site's `[node.sites]` preference.
async fn site_project_root(state: &DaemonState, name: &str) -> Result<PathBuf, Response> {
    let root = {
        let router = state.router.read().await;
        let root = router
            .iter()
            .find(|s| s.name() == name.to_lowercase())
            .map(|s| s.document_root().to_path_buf());
        root
    };
    let Some(root) = root else {
        return Err(Response::Error {
            code: ErrorCode::NotFound,
            message: format!("no site named '{name}'"),
        });
    };
    std::fs::canonicalize(root).map_err(|e| internal(format!("cannot resolve site root: {e}")))
}

/// Every registered site's canonical project root, for naming preference roots.
async fn site_roots(state: &DaemonState) -> Vec<(String, String)> {
    let sites: Vec<(String, PathBuf)> = state
        .router
        .read()
        .await
        .iter()
        .map(|s| (s.name().to_owned(), s.document_root().to_path_buf()))
        .collect();
    sites
        .into_iter()
        .filter_map(|(name, root)| {
            let root = std::fs::canonicalize(root).ok()?;
            Some((name, root.to_string_lossy().into_owned()))
        })
        .collect()
}

/// `ListNode` - installed releases with the default marker and, per release,
/// what uninstalling it would do. Disk only; never takes `tool_mutate`.
async fn node_versions_response(state: &DaemonState) -> Response {
    let (configured, prefs) = {
        let cfg = state.config.lock().await;
        (cfg.node.default.clone(), cfg.node.sites.clone())
    };
    let default = yerd_platform::node::default_selector(&state.dirs, configured.as_deref());
    let roots = if prefs.is_empty() {
        Vec::new()
    } else {
        site_roots(state).await
    };
    let installed = yerd_platform::node::installed(&state.dirs);
    let pairs = node_pairs(&installed);
    let default_exact = default
        .as_deref()
        .and_then(|d| yerd_core::node::select_release(d, pairs.iter().copied()).ok());
    let versions: Vec<&str> = installed.iter().map(|n| n.version.as_str()).collect();
    let updates = crate::node_updates::cached_updates(state, &versions).await;
    let releases = installed
        .iter()
        .rev()
        .map(|n| {
            let plan =
                yerd_core::node::plan_uninstall(&n.version, &pairs, default.as_deref(), &prefs);
            let cleared = plan.as_ref().map(|p| p.cleared.clone()).unwrap_or_default();
            yerd_ipc::NodeRelease {
                version: n.version.clone(),
                lts: n.lts,
                codename: n.codename.clone(),
                default: default_exact.as_deref() == Some(n.version.as_str()),
                uninstall_blocked: plan.is_err(),
                clears_preferences: cleared
                    .into_iter()
                    .map(|root| yerd_ipc::NodePreferenceRef {
                        site: roots
                            .iter()
                            .find(|(_, r)| *r == root)
                            .map(|(name, _)| name.clone()),
                        root,
                    })
                    .collect(),
                update: updates.get(&n.version).cloned(),
            }
        })
        .collect();
    Response::NodeVersions {
        installed: releases,
    }
}

/// `AvailableNode` - the published releases grouped by major, fetched on demand.
async fn available_node_response(state: &DaemonState) -> Response {
    let dl = crate::network::downloader(state).await;
    let d = time::OffsetDateTime::now_utc().date();
    let today = format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day());
    available_node_with(state, &dl, &today).await
}

/// Injectable core of [`available_node_response`].
async fn available_node_with(
    state: &DaemonState,
    dl: &dyn yerd_php::Downloader,
    today: &str,
) -> Response {
    let installed = yerd_platform::node::installed(&state.dirs);
    let versions: Vec<&str> = installed.iter().map(|n| n.version.as_str()).collect();
    match crate::tools::node::available(dl, &versions, today).await {
        Ok(majors) => {
            crate::node_updates::store(state, crate::tools::node::latest_by_major(&majors)).await;
            Response::AvailableNode { majors }
        }
        Err(e) => internal(format!("couldn't load the Node release index: {e}")),
    }
}

/// `UninstallNode` - remove one release. Config changes (a persisted legacy
/// default, a cleared default after the last release, cleared preferences) are
/// saved before any file is removed so nothing dangles.
async fn uninstall_node(version: &str, state: &DaemonState) -> Response {
    let _mutate = state.tool_mutate.lock().await;
    let target = version.strip_prefix('v').unwrap_or(version);
    if yerd_core::node::numeric_selector(target).map_or(true, |p| p.len() != 3) {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!(
                "expected an exact installed Node version such as 24.1.0, got {version:?}"
            ),
        };
    }
    let (configured, prefs) = {
        let cfg = state.config.lock().await;
        (cfg.node.default.clone(), cfg.node.sites.clone())
    };
    let default = yerd_platform::node::default_selector(&state.dirs, configured.as_deref());
    let installed = yerd_platform::node::installed(&state.dirs);
    let pairs = node_pairs(&installed);
    let plan = match yerd_core::node::plan_uninstall(target, &pairs, default.as_deref(), &prefs) {
        Ok(plan) => plan,
        Err(yerd_core::node::UninstallRefusal::NotInstalled) => {
            return Response::Error {
                code: ErrorCode::NotFound,
                message: format!("Node {target} is not installed"),
            }
        }
        Err(yerd_core::node::UninstallRefusal::IsDefault) => {
            return Response::Error {
                code: ErrorCode::InvalidPath,
                message: format!(
                    "Node {target} is the global default - set another version as default first"
                ),
            }
        }
    };
    let Some(release) = installed.iter().find(|n| n.version == target) else {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("Node {target} is not installed"),
        };
    };
    {
        let mut guard = state.config.lock().await;
        let mut new = guard.clone();
        new.node.default.clone_from(&plan.default);
        for root in &plan.cleared {
            new.node.sites.remove(root);
        }
        if new != *guard {
            if let Err(e) = new.save(&state.config_path) {
                return internal(format!("config save failed: {e}"));
            }
            *guard = new;
        }
    }
    if let Err(e) = crate::tools::node::uninstall_version(
        &state.dirs,
        release,
        installed.iter().all(|n| n.version == target),
    ) {
        return Response::Error {
            code: tool_error_code(&e),
            message: e.to_string(),
        };
    }
    reconcile_tool_shims_now(state).await;
    node_versions_response(state).await
}

/// `ClearNodeVersion` - drop a site's saved preference. Idempotent.
async fn clear_node_version(site: &str, state: &DaemonState) -> Response {
    let _mutate = state.tool_mutate.lock().await;
    let root = match site_project_root(state, site).await {
        Ok(root) => root,
        Err(response) => return response,
    };
    let key = root.to_string_lossy().into_owned();
    let mut guard = state.config.lock().await;
    if !guard.node.sites.contains_key(&key) {
        return Response::Ok;
    }
    let mut new = guard.clone();
    new.node.sites.remove(&key);
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *guard = new;
    Response::Ok
}

/// `NodeSite` - a site's effective Node version and its source. Failures are
/// reported in the reply's `error` field so the client can render them inline.
async fn node_site_response(name: &str, state: &DaemonState) -> Response {
    let (configured, prefs) = {
        let cfg = state.config.lock().await;
        (cfg.node.default.clone(), cfg.node.sites.clone())
    };
    let entry = match site_project_root(state, name).await {
        Ok(root) => resolve_node(name, &state.dirs, &root, configured.as_deref(), &prefs),
        Err(Response::Error { message, .. }) => node_error_entry(name, message),
        Err(_) => node_error_entry(name, format!("cannot resolve site '{name}'")),
    };
    Response::NodeSite {
        preference: entry.preference,
        project_file: entry.project_file,
        effective: entry.effective,
        source: entry.source,
        error: entry.error,
        missing: entry.missing,
    }
}

/// `NodeSites` - every routed site's Node resolution, in router order. Each
/// entry matches what `NodeSite` replies for that site. The per-site
/// filesystem sweep runs on the blocking pool.
async fn node_sites_response(state: &DaemonState) -> Response {
    let (configured, prefs) = {
        let cfg = state.config.lock().await;
        (cfg.node.default.clone(), cfg.node.sites.clone())
    };
    let sites: Vec<(String, PathBuf)> = state
        .router
        .read()
        .await
        .iter()
        .map(|s| (s.name().to_owned(), s.document_root().to_path_buf()))
        .collect();
    let dirs = state.dirs.clone();
    let sweep = tokio::task::spawn_blocking(move || {
        sites
            .into_iter()
            .map(|(name, root)| match std::fs::canonicalize(root) {
                Ok(root) => resolve_node(&name, &dirs, &root, configured.as_deref(), &prefs),
                Err(e) => node_error_entry(&name, format!("cannot resolve site root: {e}")),
            })
            .collect()
    })
    .await;
    match sweep {
        Ok(sites) => Response::NodeSites { sites },
        Err(e) => internal(format!("node sites task failed: {e}")),
    }
}

fn node_error_entry(site: &str, error: String) -> yerd_ipc::NodeSiteEntry {
    yerd_ipc::NodeSiteEntry {
        site: site.to_owned(),
        preference: None,
        project_file: None,
        effective: None,
        source: None,
        error: Some(error),
        missing: None,
    }
}

/// Resolve the Node release for a site at canonical project `root`, from the
/// configured global default and the per-root preferences. Shared by `NodeSite`
/// and `NodeSites` so both resolve identically.
fn resolve_node(
    site: &str,
    dirs: &yerd_platform::PlatformDirs,
    root: &Path,
    configured: Option<&str>,
    prefs: &std::collections::BTreeMap<String, String>,
) -> yerd_ipc::NodeSiteEntry {
    let preference = prefs.get(root.to_string_lossy().as_ref()).cloned();
    let file = match yerd_platform::node::project_file(root, Some(root)) {
        Ok(file) => file,
        Err(error) => {
            return yerd_ipc::NodeSiteEntry {
                preference,
                ..node_error_entry(site, error)
            }
        }
    };
    let default = yerd_platform::node::default_selector(dirs, configured);
    let chosen = yerd_core::node::choose(
        file.as_ref().map(|f| (f.selector.as_str(), f.kind)),
        preference.as_deref(),
        default.as_deref(),
    )
    .map(|(selector, _)| selector.to_owned());
    let (effective, source, error, missing) = match yerd_platform::node::select_from(
        dirs,
        file.as_ref(),
        preference.as_deref(),
        default.as_deref(),
    ) {
        Ok(selected) => (
            Some(selected.installed.version),
            Some(selected.source),
            None,
            None,
        ),
        Err(error) => (None, None, Some(error), chosen),
    };
    let project_file = file.map(|f| {
        Box::new(yerd_ipc::NodeProjectFile {
            kind: yerd_core::node::NodeSource::from(f.kind),
            selector: f.selector,
        })
    });
    yerd_ipc::NodeSiteEntry {
        site: site.to_owned(),
        preference,
        project_file,
        effective,
        source,
        error,
        missing,
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

    #[cfg(unix)]
    #[tokio::test]
    async fn dispatch_uninstall_service_prunes_client_shims() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let bin = state
            .dirs
            .data
            .join("services")
            .join("mysql")
            .join("9.7.1")
            .join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("mysqld"), b"#!/bin/sh\n").unwrap();
        std::fs::write(bin.join("mysql"), b"#!/bin/sh\n").unwrap();
        let link = state.dirs.data.join("bin").join("mysql");

        reconcile_client_shims_now(&state).await;
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());

        let response = dispatch(
            Request::UninstallService {
                service: "mysql".to_owned(),
                version: "9.7.1".to_owned(),
                purge: false,
            },
            &state,
        )
        .await;
        assert!(matches!(response, Response::Ok), "{response:?}");
        assert!(link.symlink_metadata().is_err());
    }

    #[tokio::test]
    async fn list_proxies_reports_domains_only_for_a_customised_proxy() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let target = yerd_core::UpstreamTarget::from_url_str("http://127.0.0.1:9011").unwrap();
        let plain = yerd_core::ProxySite::new("reverb", target.clone()).unwrap();
        let custom = yerd_core::ProxySite::new("app", target).unwrap();
        {
            let mut cfg = state.config.lock().await;
            cfg.proxies.push(plain.clone());
            cfg.proxies.push(custom.clone());
        }
        {
            let corp = yerd_core::Domain::parse_subpart("corp").unwrap();
            let mut router = state.router.write().await;
            router.insert_proxy(plain).unwrap();
            router
                .insert_proxy_with_domains(
                    custom,
                    vec![yerd_core::Domain::apex("app"), corp.clone()],
                    corp,
                )
                .unwrap();
        }

        match dispatch(Request::ListProxies, &state).await {
            Response::Proxies { proxies, .. } => {
                let plain = proxies.iter().find(|p| p.name == "reverb").unwrap();
                assert_eq!(plain.primary_domain, None);
                assert!(plain.domains.is_empty());
                let custom = proxies.iter().find(|p| p.name == "app").unwrap();
                assert_eq!(custom.primary_domain.as_deref(), Some("corp.test"));
                assert_eq!(custom.domains, ["app.test", "corp.test"]);
            }
            other => panic!("expected Proxies, got {other:?}"),
        }
    }

    /// A name-shadowed proxy is never inserted, so its name in the shared domain
    /// maps belongs to the site that shadowed it. Reporting it must stay empty
    /// rather than advertising the site's domains as the proxy's.
    #[tokio::test]
    async fn list_proxies_reports_nothing_for_a_name_shadowed_proxy() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let target = yerd_core::UpstreamTarget::from_url_str("http://127.0.0.1:9011").unwrap();
        let shadowed = yerd_core::ProxySite::new("app", target).unwrap();
        {
            let mut cfg = state.config.lock().await;
            cfg.proxies.push(shadowed);
        }
        {
            let corp = yerd_core::Domain::parse_subpart("corp").unwrap();
            let site = yerd_core::Site::parked("app", "/srv/app", yerd_core::PhpVersion::new(8, 3))
                .unwrap();
            let mut router = state.router.write().await;
            router
                .insert_with_domains(
                    site,
                    vec![yerd_core::Domain::apex("app"), corp.clone()],
                    corp,
                )
                .unwrap();
        }

        match dispatch(Request::ListProxies, &state).await {
            Response::Proxies { proxies, .. } => {
                let shadowed = proxies.iter().find(|p| p.name == "app").unwrap();
                assert_eq!(shadowed.primary_domain, None);
                assert!(shadowed.domains.is_empty());
            }
            other => panic!("expected Proxies, got {other:?}"),
        }
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

        state.mail_store.append(SAMPLE_EML, None).await.unwrap();
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
            .append(
                b"From: a@b.c\r\nTo: d@e.f\r\nSubject: Hi\r\n\r\nbody\r\n",
                None,
            )
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
    async fn linking_a_site_routes_its_preconfigured_tunnel_hostname() {
        let tmp = tempfile::tempdir().unwrap();
        let docroot = tmp.path().join("blog");
        std::fs::create_dir_all(&docroot).unwrap();
        let state = state_in(tmp.path());
        state
            .config
            .lock()
            .await
            .tunnel
            .sites
            .insert("blog".into(), "blog.example.com".into());
        crate::tunnel::rebuild_routes(&state).await;
        assert!(state.tunnel_routes.read().await.is_empty());

        let ok = dispatch(
            Request::Link {
                name: "blog".into(),
                path: docroot,
            },
            &state,
        )
        .await;

        assert!(matches!(ok, Response::Ok), "got {ok:?}");
        assert_eq!(
            state
                .tunnel_routes
                .read()
                .await
                .get("blog.example.com")
                .map(|r| r.site.as_str()),
            Some("blog")
        );
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
    async fn set_site_favourite_lists_and_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let sites_root = tmp.path().join("sites");
        std::fs::create_dir_all(sites_root.join("blog")).unwrap();
        std::fs::create_dir_all(sites_root.join("shop")).unwrap();
        let state = state_in(tmp.path());
        dispatch(Request::Park { path: sites_root }, &state).await;

        let resp = dispatch(
            Request::SetSiteFavourite {
                site: "Blog".into(),
                favourite: true,
            },
            &state,
        )
        .await;
        assert!(matches!(resp, Response::Ok), "got {resp:?}");

        match dispatch(Request::ListSites, &state).await {
            Response::Sites { sites } => {
                let fav = |name: &str| {
                    sites
                        .iter()
                        .find(|s| s.site.name() == name)
                        .unwrap()
                        .favourite
                };
                assert!(fav("blog"));
                assert!(!fav("shop"));
            }
            other => panic!("expected Sites, got {other:?}"),
        }
        let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
        assert!(reloaded.favourites.sites.contains("blog"));

        let resp = dispatch(
            Request::SetSiteFavourite {
                site: "blog".into(),
                favourite: false,
            },
            &state,
        )
        .await;
        assert!(matches!(resp, Response::Ok), "got {resp:?}");
        let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
        assert!(reloaded.favourites.is_empty());
    }

    #[tokio::test]
    async fn list_sites_reflects_favourite_set_and_clear_without_config_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let sites_root = tmp.path().join("sites");
        std::fs::create_dir_all(sites_root.join("blog")).unwrap();
        let state = state_in(tmp.path());
        dispatch(Request::Park { path: sites_root }, &state).await;

        for favourite in [true, false] {
            let resp = dispatch(
                Request::SetSiteFavourite {
                    site: "blog".into(),
                    favourite,
                },
                &state,
            )
            .await;
            assert!(matches!(resp, Response::Ok), "got {resp:?}");

            let _config = state.config.lock().await;
            let listed = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                dispatch(Request::ListSites, &state),
            )
            .await
            .expect("ListSites must not wait on the config mutex");
            match listed {
                Response::Sites { sites } => {
                    let blog = sites.iter().find(|s| s.site.name() == "blog").unwrap();
                    assert_eq!(blog.favourite, favourite);
                }
                other => panic!("expected Sites, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn list_sites_reports_framework() {
        let tmp = tempfile::tempdir().unwrap();
        let sites_root = tmp.path().join("sites");
        std::fs::create_dir_all(sites_root.join("api")).unwrap();
        std::fs::write(
            sites_root.join("api").join("composer.json"),
            br#"{"require":{"symfony/framework-bundle":"^7"}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(sites_root.join("plain")).unwrap();
        let state = state_in(tmp.path());
        dispatch(Request::Park { path: sites_root }, &state).await;

        match dispatch(Request::ListSites, &state).await {
            Response::Sites { sites } => {
                let framework = |name: &str| {
                    sites
                        .iter()
                        .find(|s| s.site.name() == name)
                        .unwrap()
                        .framework
                };
                assert_eq!(framework("api"), Some(yerd_core::SiteFramework::Symfony));
                assert_eq!(framework("plain"), None);
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
                lan_ip,
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
                assert_eq!(lan_ip, None, "LAN off by default -> no LAN IP");
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

    /// The doctor's view of the hand-edited override file is assembled by the
    /// daemon, so the wiring - registry walk, path, read - only shows up here.
    #[tokio::test]
    async fn dispatch_diagnose_flags_a_reserved_key_in_the_local_override_file() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let path = yerd_services::version::local_override_path(&state.dirs, "mysql", "cnf");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"[mysqld]\nbind-address = 0.0.0.0\n").unwrap();

        match dispatch(Request::Diagnose, &state).await {
            Response::Diagnoses { items } => {
                let finding = items
                    .iter()
                    .find(|d| d.code == yerd_ipc::DiagnosisCode::ServiceOverrideInvalid)
                    .expect("override finding present");
                assert!(finding.detail.contains("bind-address"), "{finding:?}");
                assert!(finding.detail.contains("50-local.cnf"), "{finding:?}");
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

    #[tokio::test]
    async fn uninstall_php_removes_xdebug_files_and_entry_only_for_that_version() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let v84 = PhpVersion::new(8, 4);
        let v85 = PhpVersion::new(8, 5);
        fake_install(&state.dirs, v84);
        fake_install(&state.dirs, v85);
        dispatch(Request::SetDefaultPhp { version: v84 }, &state).await;
        let on = yerd_core::XdebugSettings {
            enabled: true,
            ..yerd_core::XdebugSettings::default()
        };
        {
            let mut cfg = state.config.lock().await;
            cfg.php.xdebug.insert(v84, on.clone());
            cfg.php.xdebug.insert(v85, on.clone());
        }
        let so = crate::ext_install::xdebug_so_path(&state.dirs, v85);
        let ini = crate::xdebug::debug_ini_path(&state.dirs, v85);
        std::fs::create_dir_all(so.parent().unwrap()).unwrap();
        std::fs::write(&so, b"so").unwrap();
        std::fs::write(&ini, b"ini").unwrap();

        let resp = dispatch(Request::UninstallPhp { version: v85 }, &state).await;
        assert!(matches!(resp, Response::PhpVersions { .. }), "{resp:?}");
        assert!(!so.exists());
        assert!(!ini.exists());
        let cfg = state.config.lock().await;
        assert_eq!(
            cfg.php.xdebug.keys().copied().collect::<Vec<_>>(),
            vec![v84]
        );
        let saved = yerd_config::Config::load(&state.config_path).unwrap();
        assert_eq!(
            saved.php.xdebug.keys().copied().collect::<Vec<_>>(),
            vec![v84]
        );
    }

    #[tokio::test]
    async fn uninstall_php_reports_a_failed_xdebug_file_removal_and_a_retry_finishes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let v84 = PhpVersion::new(8, 4);
        let v85 = PhpVersion::new(8, 5);
        fake_install(&state.dirs, v84);
        fake_install(&state.dirs, v85);
        state.config.lock().await.php.default = v84;
        let so = crate::ext_install::xdebug_so_path(&state.dirs, v85);
        std::fs::create_dir_all(so.join("busy")).unwrap();

        let resp = dispatch(Request::UninstallPhp { version: v85 }, &state).await;
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::Internal);
                assert!(message.starts_with("PHP 8.5 was removed"), "{message}");
                assert!(message.contains("couldn't remove"), "{message}");
                assert!(message.contains("`yerd uninstall php 8.5`"), "{message}");
            }
            other => panic!("expected an error, got {other:?}"),
        }
        assert!(state.config.lock().await.php.xdebug.is_empty());

        std::fs::remove_dir_all(&so).unwrap();
        std::fs::write(&so, b"so").unwrap();
        let retry = dispatch(Request::UninstallPhp { version: v85 }, &state).await;
        assert!(matches!(retry, Response::PhpVersions { .. }), "{retry:?}");
        assert!(std::fs::symlink_metadata(&so).is_err());

        let again = dispatch(Request::UninstallPhp { version: v85 }, &state).await;
        assert!(
            matches!(
                again,
                Response::Error {
                    code: ErrorCode::NotFound,
                    ..
                }
            ),
            "{again:?}"
        );
    }

    #[tokio::test]
    async fn uninstall_php_cleans_stray_xdebug_files_of_a_version_that_is_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let v84 = PhpVersion::new(8, 4);
        let v85 = PhpVersion::new(8, 5);
        fake_install(&state.dirs, v84);
        let so = crate::ext_install::xdebug_so_path(&state.dirs, v85);
        std::fs::create_dir_all(so.join("busy")).unwrap();

        let resp = dispatch(Request::UninstallPhp { version: v85 }, &state).await;
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::Internal);
                assert!(
                    message.starts_with("couldn't remove leftover Xdebug files for PHP 8.5"),
                    "{message}"
                );
                assert!(!message.contains("was removed"), "{message}");
                assert!(message.contains("`yerd uninstall php 8.5`"), "{message}");
            }
            other => panic!("expected an error, got {other:?}"),
        }

        std::fs::remove_dir_all(&so).unwrap();
        std::fs::write(&so, b"so").unwrap();
        let retry = dispatch(Request::UninstallPhp { version: v85 }, &state).await;
        match retry {
            Response::PhpVersions { installed, .. } => assert_eq!(installed, vec![v84]),
            other => panic!("expected PhpVersions, got {other:?}"),
        }
        assert!(std::fs::symlink_metadata(&so).is_err());

        let again = dispatch(Request::UninstallPhp { version: v85 }, &state).await;
        assert!(
            matches!(
                again,
                Response::Error {
                    code: ErrorCode::NotFound,
                    ..
                }
            ),
            "{again:?}"
        );
    }

    #[tokio::test]
    async fn uninstall_php_reports_an_xdebug_leftover_it_cannot_inspect() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let v84 = PhpVersion::new(8, 4);
        let v85 = PhpVersion::new(8, 5);
        fake_install(&state.dirs, v84);
        let so = crate::ext_install::xdebug_so_path(&state.dirs, v85);
        let ext_dir = so.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(ext_dir.parent().unwrap()).unwrap();
        std::fs::write(&ext_dir, b"not a directory").unwrap();

        assert!(xdebug_files_left(&state.dirs, v85).is_err());
        let resp = dispatch(Request::UninstallPhp { version: v85 }, &state).await;
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::Internal);
                assert!(
                    message.starts_with("couldn't check for leftover Xdebug files for PHP 8.5"),
                    "{message}"
                );
                assert!(message.contains(&so.display().to_string()), "{message}");
                assert!(message.contains("`yerd uninstall php 8.5`"), "{message}");
            }
            other => panic!("expected an error, got {other:?}"),
        }

        std::fs::remove_file(&ext_dir).unwrap();
        assert_eq!(xdebug_files_left(&state.dirs, v85), Ok(false));
        let again = dispatch(Request::UninstallPhp { version: v85 }, &state).await;
        assert!(
            matches!(
                again,
                Response::Error {
                    code: ErrorCode::NotFound,
                    ..
                }
            ),
            "{again:?}"
        );
    }

    #[tokio::test]
    async fn uninstall_php_reports_a_failed_xdebug_cleanup_and_a_retry_finishes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = state_in(tmp.path());
        let v84 = PhpVersion::new(8, 4);
        let v85 = PhpVersion::new(8, 5);
        fake_install(&state.dirs, v84);
        fake_install(&state.dirs, v85);
        state.config.lock().await.php.default = v84;
        state.config.lock().await.php.xdebug.insert(
            v85,
            yerd_core::XdebugSettings {
                enabled: true,
                ..yerd_core::XdebugSettings::default()
            },
        );
        let good_path = state.config_path.clone();
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, b"file").unwrap();
        state.config_path = blocker.join("config.toml");

        let resp = dispatch(Request::UninstallPhp { version: v85 }, &state).await;
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::Internal);
                assert!(
                    message.contains("dropping its Xdebug settings failed"),
                    "{message}"
                );
                assert!(message.contains("`yerd uninstall php 8.5`"), "{message}");
            }
            other => panic!("expected an error, got {other:?}"),
        }
        assert!(state.config.lock().await.php.xdebug.contains_key(&v85));

        let leftover = dispatch(Request::UninstallPhp { version: v85 }, &state).await;
        match leftover {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::Internal);
                assert!(
                    message.starts_with("couldn't remove leftover Xdebug files for PHP 8.5"),
                    "{message}"
                );
                assert!(!message.contains("was removed"), "{message}");
                assert!(message.contains("`yerd uninstall php 8.5`"), "{message}");
            }
            other => panic!("expected an error, got {other:?}"),
        }

        state.config_path = good_path;
        let retry = dispatch(Request::UninstallPhp { version: v85 }, &state).await;
        assert!(matches!(retry, Response::PhpVersions { .. }), "{retry:?}");
        assert!(state.config.lock().await.php.xdebug.is_empty());
        let saved = yerd_config::Config::load(&state.config_path).unwrap();
        assert!(saved.php.xdebug.is_empty());

        let again = dispatch(Request::UninstallPhp { version: v85 }, &state).await;
        assert!(
            matches!(
                again,
                Response::Error {
                    code: ErrorCode::NotFound,
                    ..
                }
            ),
            "{again:?}"
        );
    }

    #[tokio::test]
    async fn list_php_reports_xdebug_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let v84 = PhpVersion::new(8, 4);
        fake_install(&state.dirs, v84);
        fake_install(&state.dirs, PhpVersion::new(7, 4));
        let on = yerd_core::XdebugSettings {
            enabled: true,
            ..yerd_core::XdebugSettings::default()
        };
        state.config.lock().await.php.xdebug.insert(v84, on.clone());
        match dispatch(Request::ListPhp, &state).await {
            Response::PhpVersions {
                xdebug,
                xdebug_versions,
                ..
            } => {
                assert_eq!(xdebug.get(&v84), Some(&on));
                let (os, arch) = crate::ext_install::host_os_arch();
                let want: Vec<PhpVersion> = if yerd_core::php_xdebug::is_available(v84, os, arch) {
                    vec![v84]
                } else {
                    Vec::new()
                };
                assert_eq!(*xdebug_versions, want);
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }
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

    async fn set_global_setting(state: &DaemonState, name: &str, value: &str) -> Response {
        dispatch(
            Request::SetPhpSettings {
                settings: std::collections::BTreeMap::from([(name.to_string(), value.to_string())]),
            },
            state,
        )
        .await
    }

    struct FfiOnlyForRunner {
        php_bin: std::path::PathBuf,
    }

    #[async_trait::async_trait]
    impl yerd_php::CommandRunner for FfiOnlyForRunner {
        async fn run(&self, cmd: std::process::Command) -> std::io::Result<yerd_php::ProbeOutput> {
            let stdout = if std::path::Path::new(cmd.get_program()) == self.php_bin {
                "[PHP Modules]\nCore\nFFI\n"
            } else {
                "[PHP Modules]\nCore\n"
            };
            Ok(yerd_php::ProbeOutput {
                status_ok: true,
                stdout: stdout.to_owned(),
                stderr: String::new(),
            })
        }
    }

    #[tokio::test]
    async fn list_php_reports_ffi_versions_and_libvips() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = state_in(tmp.path());
        let v84 = PhpVersion::new(8, 4);
        fake_install_patch(&state.dirs, v84, "8.4.1");
        fake_install_patch(&state.dirs, PhpVersion::new(7, 4), "7.4.33");
        state.ffi_cache = Arc::new(crate::ffi_cache::FfiCache::new(Arc::new(
            FfiOnlyForRunner {
                php_bin: crate::php_install::cli_binary_path(&state.dirs, v84),
            },
        )));
        state.libvips_probe = Arc::new(yerd_platform::FakeLibvipsProbe { found: Some(false) });

        match dispatch(Request::ListPhp, &state).await {
            Response::PhpVersions {
                installed,
                ffi_versions,
                libvips_found,
                ..
            } => {
                assert_eq!(installed.len(), 2, "{installed:?}");
                assert_eq!(*ffi_versions, vec![v84]);
                assert_eq!(libvips_found, Some(false));
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }
    }

    fn render_pool_for(
        state: &DaemonState,
        global: &std::collections::BTreeMap<String, String>,
        overrides: &std::collections::BTreeMap<
            PhpVersion,
            std::collections::BTreeMap<String, String>,
        >,
        v: PhpVersion,
    ) -> String {
        let empty = std::collections::BTreeMap::new();
        let listen = yerd_php::Listen::UnixSocket(state.dirs.runtime.join("fpm.sock"));
        let mut cfg = yerd_php::PoolConfig::dev_defaults(v, listen, &state.dirs, 1);
        cfg.ini =
            yerd_core::php_settings::merge_effective(global, overrides.get(&v).unwrap_or(&empty))
                .into_iter()
                .collect();
        yerd_php::pure::fpm_conf::render_fpm_conf(&cfg)
    }

    #[tokio::test]
    async fn manager_overrides_drop_ffi_for_a_build_without_it() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = state_in(tmp.path());
        let with_ffi = PhpVersion::new(8, 5);
        let without_ffi = PhpVersion::new(8, 4);
        fake_install_patch(&state.dirs, with_ffi, "8.5.1");
        fake_install_patch(&state.dirs, without_ffi, "8.4.1");
        state.ffi_cache = Arc::new(crate::ffi_cache::FfiCache::new(Arc::new(
            FfiOnlyForRunner {
                php_bin: crate::php_install::cli_binary_path(&state.dirs, with_ffi),
            },
        )));
        let global =
            std::collections::BTreeMap::from([("ffi.enable".to_string(), "On".to_string())]);

        let gated =
            manager_ini_overrides(&state, &global, &std::collections::BTreeMap::new()).await;

        assert!(render_pool_for(&state, &global, &gated, with_ffi)
            .contains("php_admin_value[ffi.enable] = true"));
        assert!(!render_pool_for(&state, &global, &gated, without_ffi).contains("ffi.enable"));
    }

    #[tokio::test]
    async fn ffi_enable_override_is_refused_for_legacy_but_clearable() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let v81 = PhpVersion::new(8, 1);
        fake_install(&state.dirs, v81);

        match set_version_setting(&state, v81, "ffi.enable", "on").await {
            Response::Error { message, .. } => {
                assert!(message.contains("not available for PHP 8.1"), "{message}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
        assert!(matches!(
            set_version_setting(&state, v81, "ffi.enable", "").await,
            Response::PhpVersions { .. }
        ));
    }

    #[tokio::test]
    async fn set_php_settings_ffi_enable_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        match set_global_setting(&state, "ffi.enable", "on").await {
            Response::PhpVersions { settings, .. } => {
                assert_eq!(settings.get("ffi.enable").map(String::as_str), Some("On"));
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
                .get("ffi.enable")
                .cloned(),
            Some("On".to_string())
        );

        assert!(matches!(
            set_global_setting(&state, "ffi.enable", "preload").await,
            Response::Error { .. }
        ));

        match set_global_setting(&state, "ffi.enable", "").await {
            Response::PhpVersions { settings, .. } => {
                assert!(!settings.contains_key("ffi.enable"));
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ffi_enable_per_version_override_and_directive_refusal() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let v83 = PhpVersion::new(8, 3);
        fake_install(&state.dirs, v83);

        match set_version_setting(&state, v83, "ffi.enable", "off").await {
            Response::PhpVersions {
                version_settings, ..
            } => {
                let map = version_settings.get(&v83).expect("8.3 overrides present");
                assert_eq!(map.get("ffi.enable").map(String::as_str), Some("Off"));
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }

        match set_directive(&state, v83, "ffi.enable", "true").await {
            Response::Error { message, .. } => {
                assert!(message.contains("Allow FFI in web requests"), "{message}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// Dispatch a single-entry `SetPhpVersionSettings` request.
    async fn set_version_setting(
        state: &DaemonState,
        version: PhpVersion,
        name: &str,
        value: &str,
    ) -> Response {
        dispatch(
            Request::SetPhpVersionSettings {
                version,
                settings: std::collections::BTreeMap::from([(name.to_string(), value.to_string())]),
            },
            state,
        )
        .await
    }

    /// Dispatch a single-entry `SetPhpDirectives` request.
    async fn set_directive(
        state: &DaemonState,
        version: PhpVersion,
        name: &str,
        value: &str,
    ) -> Response {
        dispatch(
            Request::SetPhpDirectives {
                version,
                directives: std::collections::BTreeMap::from([(
                    name.to_string(),
                    value.to_string(),
                )]),
            },
            state,
        )
        .await
    }

    #[tokio::test]
    async fn set_php_version_settings_persists_canonicalises_and_falls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let v83 = PhpVersion::new(8, 3);
        fake_install(&state.dirs, v83);

        assert!(matches!(
            set_version_setting(&state, PhpVersion::new(8, 5), "memory_limit", "1G").await,
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));

        let _ = set_version_setting(&state, v83, "memory_limit", "1G").await;
        match set_version_setting(&state, v83, "display_errors", "off").await {
            Response::PhpVersions {
                version_settings, ..
            } => {
                let map = version_settings.get(&v83).expect("8.3 overrides present");
                assert_eq!(map.get("memory_limit").map(String::as_str), Some("1G"));
                assert_eq!(map.get("display_errors").map(String::as_str), Some("Off"));
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }

        assert!(matches!(
            set_version_setting(&state, v83, "memory_limit", "bogus").await,
            Response::Error { .. }
        ));
        assert!(matches!(
            set_version_setting(&state, v83, "allow_url_fopen", "1").await,
            Response::Error { .. }
        ));
        assert_eq!(
            state
                .config
                .lock()
                .await
                .php
                .version_settings
                .get(&v83)
                .and_then(|m| m.get("memory_limit"))
                .map(String::as_str),
            Some("1G")
        );

        let _ = set_version_setting(&state, v83, "memory_limit", "").await;
        match set_version_setting(&state, v83, "display_errors", "").await {
            Response::PhpVersions {
                version_settings, ..
            } => {
                assert!(
                    !version_settings.contains_key(&v83),
                    "emptied override map must drop the version key"
                );
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_php_directives_persists_rejects_reserved_and_removes() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let v83 = PhpVersion::new(8, 3);
        fake_install(&state.dirs, v83);

        assert!(matches!(
            set_directive(&state, PhpVersion::new(8, 5), "xdebug.mode", "debug").await,
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));

        match set_directive(&state, v83, "xdebug.mode", "debug").await {
            Response::PhpVersions { directives, .. } => {
                assert_eq!(
                    directives
                        .get(&v83)
                        .and_then(|m| m.get("xdebug.mode"))
                        .map(String::as_str),
                    Some("debug")
                );
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }

        for (name, value) in [
            ("memory_limit", "1G"),
            ("extension", "/evil.so"),
            ("zend_extension", "/evil.so"),
            ("openssl.cafile", "/evil.pem"),
            ("1bad", "x"),
            ("bad name", "x"),
            ("xdebug.mode", "debug; evil"),
        ] {
            assert!(
                matches!(
                    set_directive(&state, v83, name, value).await,
                    Response::Error {
                        code: ErrorCode::InvalidPath,
                        ..
                    }
                ),
                "{name}={value} should be rejected"
            );
        }
        assert_eq!(
            state
                .config
                .lock()
                .await
                .php
                .directives
                .get(&v83)
                .map(std::collections::BTreeMap::len),
            Some(1)
        );

        match set_directive(&state, v83, "xdebug.mode", "").await {
            Response::PhpVersions { directives, .. } => {
                assert!(
                    !directives.contains_key(&v83),
                    "emptied directive map must drop the version key"
                );
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }
    }

    async fn set_pool(
        state: &DaemonState,
        version: PhpVersion,
        name: &str,
        value: &str,
    ) -> Response {
        dispatch(
            Request::SetPhpPoolSettings {
                version,
                settings: std::collections::BTreeMap::from([(name.to_string(), value.to_string())]),
            },
            state,
        )
        .await
    }

    #[tokio::test]
    async fn set_php_pool_settings_persists_validates_and_removes() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let v83 = PhpVersion::new(8, 3);
        fake_install(&state.dirs, v83);

        assert!(matches!(
            set_pool(&state, PhpVersion::new(8, 5), "max_children", "32").await,
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));

        match set_pool(&state, v83, "max_children", "32").await {
            Response::PhpVersions { pool, .. } => {
                assert_eq!(
                    pool.get(&v83)
                        .and_then(|m| m.get("max_children"))
                        .map(String::as_str),
                    Some("32")
                );
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }

        for (name, value) in [
            ("max_children", "0"),
            ("max_children", "1025"),
            ("max_children", "abc"),
            ("max_children", "-1"),
            ("start_servers", "4"),
            ("pm.max_children", "32"),
        ] {
            assert!(
                matches!(
                    set_pool(&state, v83, name, value).await,
                    Response::Error {
                        code: ErrorCode::InvalidPath,
                        ..
                    }
                ),
                "{name}={value} should be rejected"
            );
        }
        assert_eq!(
            state
                .config
                .lock()
                .await
                .php
                .pool
                .get(&v83)
                .and_then(|m| m.get("max_children"))
                .map(String::as_str),
            Some("32")
        );

        match set_pool(&state, v83, "max_children", "064").await {
            Response::PhpVersions { pool, .. } => {
                assert_eq!(
                    pool.get(&v83)
                        .and_then(|m| m.get("max_children"))
                        .map(String::as_str),
                    Some("64"),
                    "value must persist canonically"
                );
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }

        match set_pool(&state, v83, "max_children", "").await {
            Response::PhpVersions { pool, .. } => {
                assert!(
                    !pool.contains_key(&v83),
                    "emptied pool map must drop the version key"
                );
            }
            other => panic!("expected PhpVersions, got {other:?}"),
        }
    }

    /// The `pm.` prefix is now reserved out of the free-form directives path,
    /// so the workaround from issue #200 is refused at set time with a pointer
    /// at the pool command instead of rendering a broken `php_value` line.
    #[tokio::test]
    async fn pool_settings_are_refused_through_the_directives_path() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let v83 = PhpVersion::new(8, 3);
        fake_install(&state.dirs, v83);

        match set_directive(&state, v83, "pm.max_children", "32").await {
            Response::Error {
                code: ErrorCode::InvalidPath,
                message,
            } => assert!(message.contains("yerd php pool"), "got: {message}"),
            other => panic!("expected InvalidPath, got {other:?}"),
        }
        assert!(state.config.lock().await.php.directives.is_empty());
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
    async fn remove_php_extension_waits_for_php_settings_mutate() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        state.config.lock().await.php.extensions.insert(
            PhpVersion::new(8, 5),
            vec![yerd_config::ExtEntry {
                name: "scrypt".to_string(),
                path: "/a/scrypt.so".to_string(),
                zend: false,
            }],
        );
        let remove = || {
            dispatch(
                Request::RemovePhpExtension {
                    version: PhpVersion::new(8, 5),
                    name: "scrypt".to_string(),
                },
                &state,
            )
        };

        let guard = state.php_settings_mutate.lock().await;
        let blocked = tokio::time::timeout(std::time::Duration::from_millis(100), remove()).await;
        assert!(blocked.is_err(), "remove must wait for php_settings_mutate");
        assert!(!state.config.lock().await.php.extensions.is_empty());
        drop(guard);

        let resp = remove().await;
        assert!(matches!(resp, Response::PhpExtensions { .. }), "{resp:?}");
        assert!(state.config.lock().await.php.extensions.is_empty());
    }

    #[tokio::test]
    async fn add_php_extension_refuses_xdebug_while_managed_xdebug_is_on() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let v = PhpVersion::new(8, 4);
        fake_install(&state.dirs, v);
        state.config.lock().await.php.xdebug.insert(
            v,
            yerd_core::XdebugSettings {
                enabled: true,
                ..yerd_core::XdebugSettings::default()
            },
        );
        let resp = dispatch(
            Request::AddPhpExtension {
                version: v,
                path: "/opt/ext/xdebug.so".to_string(),
                name: None,
                zend: true,
            },
            &state,
        )
        .await;
        match resp {
            Response::Error {
                code: ErrorCode::AlreadyExists,
                message,
            } => assert!(message.contains("yerd xdebug"), "got: {message}"),
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
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
            } else if url.contains(".json") {
                Ok(self.manifest.clone().into_bytes())
            } else {
                Err(yerd_php::DownloadError::Transport {
                    url: url.to_owned(),
                    reason: "unexpected".into(),
                })
            }
        }
    }

    /// A `php.json` body with the given `(php, minor, revision)` builds for the
    /// host platform. Tarball shas are placeholders (`"00"`) - the poll /
    /// available paths never download tarballs.
    fn listing_body(builds: &[(&str, &str, u32)]) -> String {
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
        format!("{{ \"schema\": 1, \"builds\": [{}] }}", entries.join(","))
    }

    /// Build + sign a `php.json` for the host platform (see [`listing_body`]).
    fn signed_listing(builds: &[(&str, &str, u32)]) -> crate::test_support::SignedManifest {
        crate::test_support::sign_manifest(&listing_body(builds))
    }

    /// Routes stable `php.json` to `stable` and legacy `php-legacy.json` to
    /// `legacy`, so `available_php_with` sees a distinct manifest per channel.
    struct TwoChannelDl {
        stable: ListingDl,
        legacy: ListingDl,
    }
    #[async_trait::async_trait]
    impl yerd_php::Downloader for TwoChannelDl {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            if url.contains("php-legacy.json") {
                self.legacy.download(url).await
            } else {
                self.stable.download(url).await
            }
        }
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

    /// Serves a valid legacy `php-legacy.json` but errors on every stable
    /// `php.json` request, modelling a reachable legacy manifest behind an
    /// unreachable stable one.
    struct LegacyOnlyDl {
        legacy: ListingDl,
    }
    #[async_trait::async_trait]
    impl yerd_php::Downloader for LegacyOnlyDl {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            if url.contains("php-legacy.json") {
                self.legacy.download(url).await
            } else {
                Err(yerd_php::DownloadError::Transport {
                    url: url.to_owned(),
                    reason: "stable unreachable".into(),
                })
            }
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
    async fn poll_and_refresh_is_channel_aware_for_legacy() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_build(&state.dirs, PhpVersion::new(8, 5), "8.5.6", 1);
        fake_install_build(&state.dirs, PhpVersion::new(7, 4), "7.4.20", 1);
        let (stable, legacy) = crate::test_support::sign_manifest_pair(
            &listing_body(&[("8.5.9", "8.5", 1)]),
            &listing_body(&[("7.4.33", "7.4", 1)]),
        );
        let dl = TwoChannelDl {
            stable: ListingDl::new(&stable),
            legacy: ListingDl::new(&legacy),
        };

        crate::php_updates::poll_and_refresh(&state, &dl, &stable.public_key).await;

        let cache = state.php_updates.read().await;
        assert_eq!(
            cache.get(&PhpVersion::new(8, 5)).cloned(),
            Some(("8.5.9".to_owned(), 1)),
            "stable minor resolved from php.json"
        );
        assert_eq!(
            cache.get(&PhpVersion::new(7, 4)).cloned(),
            Some(("7.4.33".to_owned(), 1)),
            "legacy minor resolved from php-legacy.json"
        );
    }

    #[tokio::test]
    async fn poll_and_refresh_tolerates_untrusted_legacy_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_build(&state.dirs, PhpVersion::new(8, 5), "8.5.6", 1);
        fake_install_build(&state.dirs, PhpVersion::new(7, 4), "7.4.20", 1);
        // The legacy manifest is signed with a DIFFERENT key, so its signature
        // fails verification against the stable key the poll is given - the
        // legacy fetch degrades to `None` and 7.4 is skipped, not errored.
        let stable = signed_listing(&[("8.5.9", "8.5", 1)]);
        let legacy = signed_listing(&[("7.4.33", "7.4", 1)]);
        let dl = TwoChannelDl {
            stable: ListingDl::new(&stable),
            legacy: ListingDl::new(&legacy),
        };

        crate::php_updates::poll_and_refresh(&state, &dl, &stable.public_key).await;

        let cache = state.php_updates.read().await;
        assert_eq!(
            cache.get(&PhpVersion::new(8, 5)).cloned(),
            Some(("8.5.9".to_owned(), 1)),
            "stable minor still refreshed when the legacy manifest is unreachable"
        );
        assert!(
            !cache.contains_key(&PhpVersion::new(7, 4)),
            "legacy minor is skipped, not errored"
        );
    }

    #[tokio::test]
    async fn poll_and_refresh_preserves_cached_legacy_update_when_legacy_fetch_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_build(&state.dirs, PhpVersion::new(8, 5), "8.5.6", 1);
        fake_install_build(&state.dirs, PhpVersion::new(7, 4), "7.4.20", 1);
        state
            .php_updates
            .write()
            .await
            .insert(PhpVersion::new(7, 4), ("7.4.33".to_owned(), 1));
        // Legacy is signed with a different key than the poll is given, so its
        // fetch degrades to `None`; the stable manifest is valid.
        let stable = signed_listing(&[("8.5.9", "8.5", 1)]);
        let legacy = signed_listing(&[("7.4.40", "7.4", 1)]);
        let dl = TwoChannelDl {
            stable: ListingDl::new(&stable),
            legacy: ListingDl::new(&legacy),
        };

        crate::php_updates::poll_and_refresh(&state, &dl, &stable.public_key).await;

        let cache = state.php_updates.read().await;
        assert_eq!(
            cache.get(&PhpVersion::new(8, 5)).cloned(),
            Some(("8.5.9".to_owned(), 1)),
            "stable minor is refreshed"
        );
        assert_eq!(
            cache.get(&PhpVersion::new(7, 4)).cloned(),
            Some(("7.4.33".to_owned(), 1)),
            "a legacy fetch failure preserves the previously-cached legacy update"
        );
    }

    #[tokio::test]
    async fn poll_and_refresh_checks_legacy_when_stable_is_unreachable() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_build(&state.dirs, PhpVersion::new(7, 4), "7.4.20", 1);
        let legacy = signed_listing(&[("7.4.33", "7.4", 1)]);
        let dl = LegacyOnlyDl {
            legacy: ListingDl::new(&legacy),
        };

        crate::php_updates::poll_and_refresh(&state, &dl, &legacy.public_key).await;

        let cache = state.php_updates.read().await;
        assert_eq!(
            cache.get(&PhpVersion::new(7, 4)).cloned(),
            Some(("7.4.33".to_owned(), 1)),
            "a legacy-only install still gets update checks when stable is unreachable"
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

    // A tiny `latest.json` payload. Far-future versions so the target is always
    // newer than the daemon's compiled `current` version, regardless of the
    // build.
    const LATEST_MANIFEST: &str = r#"{
        "schema": 1,
        "stable": {"tag_name":"v99.0.1","prerelease":false,"draft":false,"assets":[]},
        "rc": {"tag_name":"v99.1.0-rc.1","prerelease":true,"draft":false,"assets":[]}
    }"#;

    /// Fake CDN downloader: serves a signed `latest.json` for the manifest URL
    /// and its detached signature for the `.minisig` URL. Feed `key()` to the
    /// self-update entry points as the verifying public key.
    struct ManifestDl(crate::test_support::SignedManifest);
    impl ManifestDl {
        fn new(manifest: &str) -> Self {
            Self(crate::test_support::sign_manifest(manifest))
        }
        fn key(&self) -> &str {
            &self.0.public_key
        }
    }
    #[async_trait::async_trait]
    impl yerd_php::Downloader for ManifestDl {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            if url.ends_with(".minisig") {
                Ok(self.0.minisig.clone().into_bytes())
            } else {
                Ok(self.0.manifest.clone().into_bytes())
            }
        }
    }

    #[tokio::test]
    async fn check_update_reports_both_channel_latests_live() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let dl = ManifestDl::new(LATEST_MANIFEST);
        let resp = crate::self_update::check_update(None, &state, &dl, dl.key()).await;
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
        assert_eq!(state.yerd_update.read().await.len(), 2);
    }

    #[tokio::test]
    async fn check_update_edge_override_selects_prerelease_target() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let dl = ManifestDl::new(LATEST_MANIFEST);
        let resp =
            crate::self_update::check_update(Some(yerd_ipc::Channel::Edge), &state, &dl, dl.key())
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
    async fn check_update_rejects_tampered_manifest_signature() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        // Sign the manifest with one key, but verify against a *different* key:
        // the signature must fail and the live fetch must yield nothing.
        let dl = ManifestDl::new(LATEST_MANIFEST);
        let other = crate::test_support::sign_manifest(LATEST_MANIFEST);
        let resp = crate::self_update::check_update(None, &state, &dl, &other.public_key).await;
        match resp {
            Response::UpdateStatus {
                latest_stable,
                source,
                ..
            } => {
                assert_eq!(source, yerd_ipc::UpdateSource::Cached);
                assert_eq!(
                    latest_stable, None,
                    "a bad-signature manifest must not be trusted"
                );
            }
            other => panic!("expected UpdateStatus, got {other:?}"),
        }
        assert!(state.yerd_update.read().await.is_empty());
    }

    #[tokio::test]
    async fn check_update_falls_back_to_cache_when_offline() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let dl = ManifestDl::new(LATEST_MANIFEST);
        crate::self_update::poll_and_refresh(&state, &dl, dl.key()).await;
        let resp = crate::self_update::check_update(None, &state, &FailingDl, dl.key()).await;
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

    /// Fake downloader for the full stage flow. Serves the signed `latest.json`
    /// (and its `.minisig`) for the manifest URLs; the signed bytes (`b"test"`)
    /// for any artifact URL; the matching artifact signature for any `.sig` /
    /// `.minisig` URL; and a `SHA256SUMS`. The manifest and the artifact bytes
    /// are signed with a single key ([`StageDl::key`]), which is fed to
    /// `stage_update` to verify both the manifest and the artifact.
    struct StageDl {
        manifest: crate::test_support::SignedManifest,
        artifact_sig: String,
        sums: String,
    }
    impl StageDl {
        /// Build from a `stable` release object body and a `SHA256SUMS`. The
        /// manifest wraps `stable` as `latest.json`; the artifact bytes `b"test"`
        /// are signed with the same key so one public key verifies both.
        fn new(stable: &str, sums: String) -> Self {
            let manifest = format!(r#"{{"schema":1,"stable":{stable},"rc":null}}"#);
            let (manifest, artifact) = crate::test_support::sign_manifest_pair(&manifest, "test");
            Self {
                manifest,
                artifact_sig: artifact.minisig,
                sums,
            }
        }
        fn key(&self) -> &str {
            &self.manifest.public_key
        }
    }
    #[async_trait::async_trait]
    impl yerd_php::Downloader for StageDl {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            if url.contains("latest.json.minisig") {
                Ok(self.manifest.minisig.clone().into_bytes())
            } else if url.contains("latest.json") {
                Ok(self.manifest.manifest.clone().into_bytes())
            } else if url.ends_with("SHA256SUMS") {
                Ok(self.sums.clone().into_bytes())
            } else if url.ends_with(".minisig") || url.ends_with(".sig") {
                Ok(self.artifact_sig.clone().into_bytes())
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
        let stable = format!(
            r#"{{"tag_name":"v99.0.1","prerelease":false,"draft":false,"assets":[
                {{"name":"{mac}","browser_download_url":"https://h/{mac}","size":4}},
                {{"name":"{mac}.minisig","browser_download_url":"https://h/{mac}.minisig","size":1}},
                {{"name":"{deb}","browser_download_url":"https://h/{deb}","size":4}},
                {{"name":"{deb}.minisig","browser_download_url":"https://h/{deb}.minisig","size":1}},
                {{"name":"{arm}","browser_download_url":"https://h/{arm}","size":4}},
                {{"name":"{arm}.minisig","browser_download_url":"https://h/{arm}.minisig","size":1}},
                {{"name":"{pkg}","browser_download_url":"https://h/{pkg}","size":4}},
                {{"name":"{pkg}.minisig","browser_download_url":"https://h/{pkg}.minisig","size":1}},
                {{"name":"{pkg_arm}","browser_download_url":"https://h/{pkg_arm}","size":4}},
                {{"name":"{pkg_arm}.minisig","browser_download_url":"https://h/{pkg_arm}.minisig","size":1}},
                {{"name":"{rpm}","browser_download_url":"https://h/{rpm}","size":4}},
                {{"name":"{rpm}.minisig","browser_download_url":"https://h/{rpm}.minisig","size":1}},
                {{"name":"{rpm_arm}","browser_download_url":"https://h/{rpm_arm}","size":4}},
                {{"name":"{rpm_arm}.minisig","browser_download_url":"https://h/{rpm_arm}.minisig","size":1}},
                {{"name":"SHA256SUMS","browser_download_url":"https://h/SHA256SUMS","size":1}}
            ]}}"#
        );
        let h = yerd_update::sha256_hex(b"test");
        let sums = format!(
            "{h}  {mac}\n{h}  {deb}\n{h}  {arm}\n{h}  {pkg}\n{h}  {pkg_arm}\n{h}  {rpm}\n{h}  {rpm_arm}\n"
        );
        let dl = StageDl::new(&stable, sums);

        let resp = crate::self_update::stage_update(None, &state, &dl, dl.key()).await;
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
                let sibling = p.with_file_name(format!(
                    "{}.minisig",
                    p.file_name().and_then(|n| n.to_str()).unwrap()
                ));
                assert!(
                    sibling.exists(),
                    "staged .minisig sibling should exist at {}",
                    sibling.display()
                );
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

    /// A pre-N release layout carries only `.sig` signatures. An N-built client
    /// must still stage from it via the `select_asset` `.sig` fallback, and it
    /// always writes the sibling under the new `.minisig` name.
    #[tokio::test]
    async fn stage_update_stages_from_legacy_sig_only_layout() {
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
        let stable = format!(
            r#"{{"tag_name":"v99.0.1","prerelease":false,"draft":false,"assets":[
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
            ]}}"#
        );
        let h = yerd_update::sha256_hex(b"test");
        let sums = format!(
            "{h}  {mac}\n{h}  {deb}\n{h}  {arm}\n{h}  {pkg}\n{h}  {pkg_arm}\n{h}  {rpm}\n{h}  {rpm_arm}\n"
        );
        let dl = StageDl::new(&stable, sums);

        match crate::self_update::stage_update(None, &state, &dl, dl.key()).await {
            Response::Staged { path, .. } => {
                let p = std::path::Path::new(&path);
                assert!(p.exists(), "staged file should exist at {path}");
                let sibling = p.with_file_name(format!(
                    "{}.minisig",
                    p.file_name().and_then(|n| n.to_str()).unwrap()
                ));
                assert!(
                    sibling.exists(),
                    "staged sibling should be written as .minisig even from a .sig layout"
                );
            }
            other => panic!("expected Staged from legacy .sig layout, got {other:?}"),
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
        let rpm = "Yerd_Linux_x86_64_v99-0-1.rpm";
        let rpm_arm = "Yerd_Linux_Arm64_v99-0-1.rpm";
        let stable = format!(
            r#"{{"tag_name":"v99.0.1","prerelease":false,"draft":false,"assets":[
                {{"name":"{mac}","browser_download_url":"https://h/{mac}","size":4}},
                {{"name":"{mac}.minisig","browser_download_url":"https://h/{mac}.minisig","size":1}},
                {{"name":"{deb}","browser_download_url":"https://h/{deb}","size":4}},
                {{"name":"{deb}.minisig","browser_download_url":"https://h/{deb}.minisig","size":1}},
                {{"name":"{arm}","browser_download_url":"https://h/{arm}","size":4}},
                {{"name":"{arm}.minisig","browser_download_url":"https://h/{arm}.minisig","size":1}},
                {{"name":"{pkg}","browser_download_url":"https://h/{pkg}","size":4}},
                {{"name":"{pkg}.minisig","browser_download_url":"https://h/{pkg}.minisig","size":1}},
                {{"name":"{pkg_arm}","browser_download_url":"https://h/{pkg_arm}","size":4}},
                {{"name":"{pkg_arm}.minisig","browser_download_url":"https://h/{pkg_arm}.minisig","size":1}},
                {{"name":"{rpm}","browser_download_url":"https://h/{rpm}","size":4}},
                {{"name":"{rpm}.minisig","browser_download_url":"https://h/{rpm}.minisig","size":1}},
                {{"name":"{rpm_arm}","browser_download_url":"https://h/{rpm_arm}","size":4}},
                {{"name":"{rpm_arm}.minisig","browser_download_url":"https://h/{rpm_arm}.minisig","size":1}},
                {{"name":"SHA256SUMS","browser_download_url":"https://h/SHA256SUMS","size":1}}
            ]}}"#
        );
        let bad = "0".repeat(64);
        let sums = format!(
            "{bad}  {mac}\n{bad}  {deb}\n{bad}  {arm}\n{bad}  {pkg}\n{bad}  {pkg_arm}\n{bad}  {rpm}\n{bad}  {rpm_arm}\n"
        );
        let dl = StageDl::new(&stable, sums);
        match crate::self_update::stage_update(None, &state, &dl, dl.key()).await {
            Response::Error { message, .. } => assert!(
                message.contains("checksum verification failed"),
                "expected a checksum verification failure, got: {message}"
            ),
            other => panic!("expected Error on checksum mismatch, got {other:?}"),
        }
        assert!(
            !state.dirs.cache.join("update").join(mac).exists()
                && !state.dirs.cache.join("update").join(deb).exists()
                && !state.dirs.cache.join("update").join(arm).exists()
                && !state.dirs.cache.join("update").join(pkg).exists()
                && !state.dirs.cache.join("update").join(pkg_arm).exists()
                && !state.dirs.cache.join("update").join(rpm).exists()
                && !state.dirs.cache.join("update").join(rpm_arm).exists(),
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
    async fn set_mcp_enabled_persists_config_and_appears_in_status() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert!(
            !state.config.lock().await.mcp_enabled,
            "seeded opt-in default off"
        );
        assert!(
            !build_status_report(&state).await.mcp_enabled,
            "status report starts off"
        );

        assert_eq!(
            dispatch(Request::SetMcpEnabled { enabled: true }, &state).await,
            Response::Ok
        );
        assert!(state.config.lock().await.mcp_enabled, "in-memory config on");
        let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
        assert!(reloaded.mcp_enabled, "persisted config on");
        assert!(
            build_status_report(&state).await.mcp_enabled,
            "status report on"
        );

        assert_eq!(
            dispatch(Request::SetMcpEnabled { enabled: false }, &state).await,
            Response::Ok
        );
        let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
        assert!(!reloaded.mcp_enabled, "persisted config back off");
        assert!(
            !build_status_report(&state).await.mcp_enabled,
            "status report back off"
        );
    }

    fn network_settings(resp: Response) -> yerd_ipc::NetworkSettings {
        match resp {
            Response::NetworkSettings { settings } => *settings,
            other => panic!("expected NetworkSettings, got {other:?}"),
        }
    }

    fn set_network(
        mode: yerd_core::network::OutboundProxyMode,
        http: Option<&str>,
        https: Option<&str>,
    ) -> Request {
        Request::SetNetworkSettings {
            mode,
            http_proxy: http.map(str::to_owned),
            https_proxy: https.map(str::to_owned),
            no_proxy: None,
        }
    }

    #[tokio::test]
    async fn network_settings_redact_and_keep_the_stored_password() {
        use yerd_core::network::OutboundProxyMode;
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        std::fs::create_dir_all(&state.dirs.data).unwrap();

        let resp = dispatch(
            set_network(
                OutboundProxyMode::Manual,
                Some("http://u:hunter2@proxy.corp:8080"),
                Some(""),
            ),
            &state,
        )
        .await;
        assert!(!serde_json::to_string(&resp).unwrap().contains("hunter2"));
        let settings = network_settings(resp);
        assert_eq!(
            settings.http_proxy.as_deref(),
            Some("http://u:***@proxy.corp:8080")
        );
        let effective = settings.effective.unwrap();
        assert_eq!(
            effective.https_proxy.as_deref(),
            Some("http://u:***@proxy.corp:8080"),
            "HTTP-only manual falls back for HTTPS"
        );
        let on_disk = std::fs::read_to_string(&state.config_path).unwrap();
        assert!(
            on_disk.contains("http://u:hunter2@proxy.corp:8080"),
            "{on_disk}"
        );

        let get = dispatch(Request::GetNetworkSettings, &state).await;
        assert!(!serde_json::to_string(&get).unwrap().contains("hunter2"));

        dispatch(
            set_network(
                OutboundProxyMode::Manual,
                Some("http://u:***@proxy.corp:8080"),
                None,
            ),
            &state,
        )
        .await;
        let on_disk = std::fs::read_to_string(&state.config_path).unwrap();
        assert!(
            on_disk.contains("http://u:hunter2@proxy.corp:8080"),
            "{on_disk}"
        );
        assert!(!on_disk.contains("***"), "{on_disk}");

        let rejected = dispatch(
            set_network(
                OutboundProxyMode::Manual,
                Some("http://u:***@otherhost:8080"),
                None,
            ),
            &state,
        )
        .await;
        match rejected {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::InvalidPath);
                assert!(message.contains("re-enter"), "{message}");
            }
            other => panic!("expected InvalidPath, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&state.config_path).unwrap(),
            on_disk,
            "a rejected save leaves the file unchanged"
        );

        let bad = dispatch(
            set_network(
                OutboundProxyMode::Manual,
                Some("socks5://u:hunter2@proxy:1080"),
                None,
            ),
            &state,
        )
        .await;
        match bad {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::InvalidPath);
                assert!(!message.contains("hunter2"), "{message}");
            }
            other => panic!("expected InvalidPath, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn network_settings_manual_without_url_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let resp = dispatch(
            set_network(yerd_core::network::OutboundProxyMode::Manual, None, None),
            &state,
        )
        .await;
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
    }

    #[tokio::test]
    async fn network_settings_off_removes_the_env_file() {
        use yerd_core::network::OutboundProxyMode;
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        std::fs::create_dir_all(&state.dirs.data).unwrap();
        let env_file = crate::network::env_file_path(&state);
        dispatch(
            set_network(OutboundProxyMode::Manual, Some("http://proxy:3128"), None),
            &state,
        )
        .await;
        assert!(env_file.is_file());
        let settings = network_settings(
            dispatch(set_network(OutboundProxyMode::Off, None, None), &state).await,
        );
        assert_eq!(settings.mode, OutboundProxyMode::Off);
        assert_eq!(settings.http_proxy.as_deref(), Some("http://proxy:3128"));
        assert!(!env_file.exists());
    }

    #[tokio::test]
    async fn set_lan_enabled_persists_config_and_appears_in_status() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert!(!state.config.lock().await.lan_enabled, "seeded off");
        assert!(!build_status_report(&state).await.lan_enabled);

        assert_eq!(
            dispatch(Request::SetLanEnabled { enabled: true }, &state).await,
            Response::Ok
        );
        assert!(state.config.lock().await.lan_enabled, "in-memory config on");
        let reloaded = yerd_config::Config::load(&state.config_path).unwrap();
        assert!(reloaded.lan_enabled, "persisted config on");
        assert!(build_status_report(&state).await.lan_enabled);

        assert_eq!(
            dispatch(Request::SetLanEnabled { enabled: false }, &state).await,
            Response::Ok
        );
        assert!(!state.config.lock().await.lan_enabled, "back off");
    }

    #[tokio::test]
    async fn disabling_lan_revokes_a_pending_remote_setup_code() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        *state.remote_setup_code.lock().await = Some(crate::state::RemoteSetupCode {
            value: "abc".to_owned(),
            expires_at: std::time::Instant::now() + std::time::Duration::from_secs(60),
            used: false,
        });
        assert_eq!(
            dispatch(Request::SetLanEnabled { enabled: false }, &state).await,
            Response::Ok
        );
        assert!(
            state.remote_setup_code.lock().await.is_none(),
            "disabling LAN clears any pending one-time code"
        );
    }

    #[tokio::test]
    async fn mint_remote_setup_code_rejects_when_lan_off() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        // LAN off by default → guarded.
        match dispatch(Request::MintRemoteSetupCode, &state).await {
            Response::Error { code, .. } => {
                assert_eq!(code, yerd_ipc::ErrorCode::LanNotReady);
            }
            other => panic!("expected LanNotReady error, got {other:?}"),
        }
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
                legacy,
            } => {
                assert_eq!(
                    available,
                    vec![PhpVersion::new(8, 3), PhpVersion::new(8, 5)]
                );
                assert_eq!(installed, vec![PhpVersion::new(8, 5)]);
                assert!(
                    legacy.is_empty(),
                    "legacy manifest unreachable via ListingDl → empty"
                );
            }
            other => panic!("expected AvailablePhp, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn available_php_lists_legacy_from_second_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let (stable, legacy) = crate::test_support::sign_manifest_pair(
            &listing_body(&[("8.4.21", "8.4", 1), ("8.5.9", "8.5", 1)]),
            &listing_body(&[("7.4.33", "7.4", 1), ("8.1.33", "8.1", 1)]),
        );
        let dl = TwoChannelDl {
            stable: ListingDl::new(&stable),
            legacy: ListingDl::new(&legacy),
        };

        match available_php_with(&state, &dl, &stable.public_key).await {
            Response::AvailablePhp {
                available, legacy, ..
            } => {
                assert_eq!(
                    available,
                    vec![PhpVersion::new(8, 4), PhpVersion::new(8, 5)]
                );
                assert_eq!(legacy, vec![PhpVersion::new(7, 4), PhpVersion::new(8, 1)]);
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

    #[test]
    fn legacy_install_gate_blocks_only_unconfirmed_legacy() {
        assert!(legacy_install_gate(PhpVersion::new(8, 5), false).is_none());
        assert!(legacy_install_gate(PhpVersion::new(7, 4), true).is_none());
        match legacy_install_gate(PhpVersion::new(7, 4), false) {
            Some(Response::Error { code, .. }) => assert_eq!(code, ErrorCode::LegacyRestricted),
            other => panic!("expected LegacyRestricted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn install_php_legacy_without_confirm_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match install_php(PhpVersion::new(7, 4), false, &state).await {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::LegacyRestricted),
            other => panic!("expected LegacyRestricted, got {other:?}"),
        }
        assert!(!state.dirs.data.join("php").join("php-7.4").exists());
    }

    #[tokio::test]
    async fn install_php_streamed_legacy_without_confirm_creates_no_job() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(state_in(tmp.path()));
        match install_php_streamed(PhpVersion::new(8, 0), false, state.clone()).await {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::LegacyRestricted),
            other => panic!("expected synchronous LegacyRestricted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_default_php_rejects_legacy_even_when_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        fake_install_patch(&state.dirs, PhpVersion::new(7, 4), "7.4.33");
        match set_default_php(PhpVersion::new(7, 4), &state).await {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::LegacyRestricted),
            other => panic!("expected LegacyRestricted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn adopt_default_if_unset_never_adopts_legacy() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let before = state.config.lock().await.php.default;
        adopt_default_if_unset(PhpVersion::new(7, 4), &state).await;
        assert_eq!(state.config.lock().await.php.default, before);
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

    #[tokio::test]
    async fn path_needs_setup_no_tools_is_some_false() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        #[cfg(unix)]
        assert_eq!(path_needs_setup(&state).await, Some(false));
        #[cfg(not(unix))]
        assert_eq!(path_needs_setup(&state).await, None);
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod node_tests {
    use super::*;
    use yerd_core::node::NodeSource;

    #[tokio::test]
    async fn selections_persist_exact_releases_and_tool_status_tracks_default() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        for version in ["22.9.0", "24.2.0"] {
            let root = yerd_platform::node::versions_dir(&state.dirs).join(version);
            let bin = root.join(format!("node-v{version}-test/bin"));
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(root.join(".version"), version).unwrap();
            for name in ["node", "npm", "npx"] {
                std::fs::write(bin.join(name), "fake").unwrap();
            }
        }
        let project = tmp.path().join("example");
        std::fs::create_dir(&project).unwrap();
        let project = std::fs::canonicalize(project).unwrap();
        state
            .router
            .write()
            .await
            .insert(
                yerd_core::Site::linked("example", &project, yerd_core::PhpVersion::new(8, 3))
                    .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            save_node_version("22", None, &state).await,
            Response::Ok
        ));
        assert!(matches!(
            save_node_version("24", Some("EXAMPLE"), &state).await,
            Response::Ok
        ));
        let cfg = yerd_config::Config::load(&state.config_path).unwrap();
        assert_eq!(cfg.node.default.as_deref(), Some("22.9.0"));
        assert_eq!(
            cfg.node
                .sites
                .get(project.to_string_lossy().as_ref())
                .map(String::as_str),
            Some("24.2.0")
        );
        assert_eq!(
            crate::tools::status(
                &state.dirs,
                crate::tools::Tool::Node,
                state.config.lock().await.node.default.as_deref()
            )
            .version
            .as_deref(),
            Some("v22.9.0")
        );
        assert!(matches!(
            save_node_version("23", None, &state).await,
            Response::Error { .. }
        ));
        assert_eq!(yerd_config::Config::load(&state.config_path).unwrap(), cfg);
        assert!(matches!(
            save_node_version("24", None, &state).await,
            Response::Ok
        ));
        assert_eq!(
            crate::tools::status(
                &state.dirs,
                crate::tools::Tool::Node,
                state.config.lock().await.node.default.as_deref()
            )
            .version
            .as_deref(),
            Some("v24.2.0")
        );
        assert!(matches!(uninstall_tool("node", &state).await, Response::Ok));
        let cfg = yerd_config::Config::load(&state.config_path).unwrap();
        assert!(cfg.node.default.is_none());
        assert!(cfg.node.sites.is_empty());
        assert!(state.config.lock().await.node.sites.is_empty());
        assert!(
            !crate::tools::status(
                &state.dirs,
                crate::tools::Tool::Node,
                state.config.lock().await.node.default.as_deref()
            )
            .installed
        );
    }

    struct NodeDistDl;

    fn node_archive(version: &str) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut tar = tar::Builder::new(encoder);
        for name in ["node", "npm", "npx"] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            header.set_size(4);
            header.set_cksum();
            tar.append_data(
                &mut header,
                format!("node-v{version}-test/bin/{name}"),
                &b"fake"[..],
            )
            .unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[async_trait::async_trait]
    impl yerd_php::Downloader for NodeDistDl {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            if url.ends_with("schedule.json") {
                return Err(yerd_php::DownloadError::Transport {
                    url: url.to_owned(),
                    reason: "offline".into(),
                });
            }
            let files = r#"["osx-arm64-tar","osx-x64-tar","linux-x64","linux-arm64"]"#;
            if url.ends_with("index.json") {
                return Ok(format!(
                    r#"[{{"version":"v25.1.0","lts":false,"files":{files}}},{{"version":"v24.2.0","lts":"Krypton","files":{files}}}]"#
                )
                .into_bytes());
            }
            let sha = crate::ext_install::sha256_hex(&node_archive("24.2.0"));
            if url.ends_with("SHASUMS256.txt") {
                return Ok(["darwin-arm64", "darwin-x64", "linux-x64", "linux-arm64"]
                    .map(|plat| format!("{sha}  node-v24.2.0-{plat}.tar.gz"))
                    .join("\n")
                    .into_bytes());
            }
            Ok(node_archive("24.2.0"))
        }
    }

    #[tokio::test]
    async fn save_node_version_resolves_aliases_to_exact_releases() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        release(&state, "20.19.0", Some("Iron"));
        release(&state, "22.12.0", Some("Jod"));
        release(&state, "25.1.0", None);
        for (selector, expected) in [
            ("lts/iron", "20.19.0"),
            ("lts/*", "22.12.0"),
            ("node", "25.1.0"),
        ] {
            assert!(matches!(
                save_node_version(selector, None, &state).await,
                Response::Ok
            ));
            assert_eq!(
                state.config.lock().await.node.default.as_deref(),
                Some(expected),
                "{selector}"
            );
        }
        assert_eq!(
            error_code(&save_node_version("lts/hydrogen", None, &state).await),
            Some(ErrorCode::NotFound)
        );
    }

    #[tokio::test]
    async fn install_tool_node_keeps_an_existing_default() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        release(&state, "22.9.0", Some("Jod"));
        set_node_config(&state, Some("22"), &[("/srv/a", "22.9.0")]).await;
        assert!(matches!(
            install_tool_with(crate::tools::Tool::Node, &state, &NodeDistDl).await,
            Response::Ok
        ));
        let cfg = state.config.lock().await;
        assert_eq!(cfg.node.default.as_deref(), Some("22"));
        assert_eq!(
            cfg.node.sites.get("/srv/a").map(String::as_str),
            Some("22.9.0")
        );
        drop(cfg);
        assert!(yerd_platform::node::resolve(&state.dirs, "24.2.0").is_ok());
    }

    #[tokio::test]
    async fn install_tool_node_without_a_default_makes_the_release_default() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        assert!(matches!(
            install_tool_with(crate::tools::Tool::Node, &state, &NodeDistDl).await,
            Response::Ok
        ));
        assert_eq!(
            state.config.lock().await.node.default.as_deref(),
            Some("24.2.0")
        );
        assert_eq!(
            yerd_config::Config::load(&state.config_path)
                .unwrap()
                .node
                .default
                .as_deref(),
            Some("24.2.0")
        );
    }

    #[tokio::test]
    async fn install_tool_node_persists_a_previous_legacy_default() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        release(&state, "22.9.0", None);
        let legacy = state.dirs.data.join("tools/node");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(".default"), "22.9.0").unwrap();
        assert!(matches!(
            install_tool_with(crate::tools::Tool::Node, &state, &NodeDistDl).await,
            Response::Ok
        ));
        assert_eq!(
            state.config.lock().await.node.default.as_deref(),
            Some("22.9.0")
        );
    }

    #[tokio::test]
    async fn install_tool_node_replaces_a_stale_legacy_default() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        let legacy = state.dirs.data.join("tools/node");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(".default"), "18.0.0").unwrap();
        assert!(matches!(
            install_tool_with(crate::tools::Tool::Node, &state, &NodeDistDl).await,
            Response::Ok
        ));
        assert_eq!(
            state.config.lock().await.node.default.as_deref(),
            Some("24.2.0")
        );
    }

    fn release(state: &DaemonState, version: &str, codename: Option<&str>) {
        let root = yerd_platform::node::versions_dir(&state.dirs).join(version);
        let bin = root.join(format!("node-v{version}-test/bin"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(root.join(".version"), version).unwrap();
        if let Some(codename) = codename {
            std::fs::write(root.join(".lts"), codename).unwrap();
        }
        for name in ["node", "npm", "npx"] {
            std::fs::write(bin.join(name), "fake").unwrap();
        }
    }

    async fn site(state: &DaemonState, tmp: &Path, name: &str) -> String {
        let project = tmp.join(name);
        std::fs::create_dir_all(&project).unwrap();
        let project = std::fs::canonicalize(project).unwrap();
        state
            .router
            .write()
            .await
            .insert(
                yerd_core::Site::linked(name, &project, yerd_core::PhpVersion::new(8, 3)).unwrap(),
            )
            .unwrap();
        project.to_string_lossy().into_owned()
    }

    async fn set_node_config(state: &DaemonState, default: Option<&str>, prefs: &[(&str, &str)]) {
        let mut cfg = state.config.lock().await;
        cfg.node.default = default.map(str::to_owned);
        cfg.node.sites = prefs
            .iter()
            .map(|(root, v)| ((*root).to_owned(), (*v).to_owned()))
            .collect();
    }

    fn versions(response: Response) -> Vec<yerd_ipc::NodeRelease> {
        match response {
            Response::NodeVersions { installed } => installed,
            other => panic!("expected NodeVersions, got {other:?}"),
        }
    }

    fn error_code(response: &Response) -> Option<ErrorCode> {
        match response {
            Response::Error { code, .. } => Some(*code),
            _ => None,
        }
    }

    #[tokio::test]
    async fn list_node_reports_codename_default_and_uninstall_effects() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        release(&state, "22.11.0", Some("Jod"));
        release(&state, "22.12.0", Some("Jod"));
        release(&state, "24.1.0", None);
        let blog = site(&state, tmp.path(), "blog").await;
        set_node_config(
            &state,
            Some("22"),
            &[
                (&blog, "22.12.0"),
                ("/srv/old", "24.1.0"),
                ("/srv/any", "22"),
            ],
        )
        .await;
        let held = state.tool_mutate.lock().await;
        let listed = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            node_versions_response(&state),
        )
        .await
        .unwrap();
        drop(held);
        let summary: Vec<_> = versions(listed)
            .into_iter()
            .map(|r| {
                (
                    r.version,
                    r.codename,
                    r.default,
                    r.uninstall_blocked,
                    r.clears_preferences,
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    "24.1.0".to_owned(),
                    None,
                    false,
                    false,
                    vec![yerd_ipc::NodePreferenceRef {
                        root: "/srv/old".to_owned(),
                        site: None,
                    }]
                ),
                (
                    "22.12.0".to_owned(),
                    Some("Jod".to_owned()),
                    true,
                    false,
                    vec![yerd_ipc::NodePreferenceRef {
                        root: blog.clone(),
                        site: Some("blog".to_owned()),
                    }]
                ),
                (
                    "22.11.0".to_owned(),
                    Some("Jod".to_owned()),
                    false,
                    false,
                    vec![]
                ),
            ]
        );
        set_node_config(&state, Some("24"), &[]).await;
        let blocked: Vec<_> = versions(node_versions_response(&state).await)
            .into_iter()
            .map(|r| (r.default, r.uninstall_blocked))
            .collect();
        assert_eq!(blocked, [(true, true), (false, false), (false, false)]);
    }

    #[tokio::test]
    async fn uninstall_node_applies_selector_aware_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        release(&state, "22.11.0", None);
        release(&state, "24.1.0", None);
        set_node_config(&state, Some("22.11"), &[]).await;
        assert_eq!(
            error_code(&uninstall_node("22.11.0", &state).await),
            Some(ErrorCode::InvalidPath)
        );
        assert_eq!(
            error_code(&uninstall_node("../24", &state).await),
            Some(ErrorCode::NotFound)
        );
        assert_eq!(
            error_code(&uninstall_node("23.0.0", &state).await),
            Some(ErrorCode::NotFound)
        );
        std::fs::remove_dir_all(yerd_platform::node::versions_dir(&state.dirs).join("24.1.0"))
            .unwrap();
        release(&state, "22.12.0", None);
        set_node_config(
            &state,
            Some("22"),
            &[("/srv/a", "22"), ("/srv/b", "22.11.0")],
        )
        .await;
        let left = versions(uninstall_node("v22.11.0", &state).await);
        assert_eq!(
            left.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(),
            ["22.12.0"]
        );
        let cfg = yerd_config::Config::load(&state.config_path).unwrap();
        assert_eq!(cfg.node.default.as_deref(), Some("22"));
        assert_eq!(
            cfg.node
                .sites
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["/srv/a"]
        );
        assert!(versions(uninstall_node("22.12.0", &state).await).is_empty());
        let cfg = yerd_config::Config::load(&state.config_path).unwrap();
        assert_eq!(cfg.node.default, None);
        assert!(cfg.node.sites.is_empty());
        assert!(yerd_platform::node::installed(&state.dirs).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn list_node_shows_a_node_installed_by_an_older_yerd() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        let legacy = state.dirs.data.join("tools/node");
        let dist = legacy.join("node-v24.17.0-darwin-arm64");
        let bin = dist.join("bin");
        let npm_cli = dist.join("lib/node_modules/npm/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&npm_cli).unwrap();
        std::fs::write(legacy.join(".version"), "v24.17.0").unwrap();
        std::fs::write(bin.join("node"), "fake").unwrap();
        std::fs::write(npm_cli.join("npm-cli.js"), "fake").unwrap();
        std::fs::write(npm_cli.join("npx-cli.js"), "fake").unwrap();
        std::os::unix::fs::symlink("../lib/node_modules/npm/bin/npm-cli.js", bin.join("npm"))
            .unwrap();
        std::os::unix::fs::symlink("../lib/node_modules/npm/bin/npx-cli.js", bin.join("npx"))
            .unwrap();
        let listed: Vec<_> = versions(node_versions_response(&state).await)
            .into_iter()
            .map(|r| (r.version, r.lts, r.default, r.uninstall_blocked))
            .collect();
        assert_eq!(listed, [("24.17.0".to_owned(), true, true, false)]);
    }

    #[tokio::test]
    async fn uninstall_node_persists_a_legacy_default() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        release(&state, "22.9.0", None);
        release(&state, "24.1.0", None);
        let legacy = state.dirs.data.join("tools/node");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(".default"), "24.1.0").unwrap();
        assert_eq!(versions(uninstall_node("22.9.0", &state).await).len(), 1);
        let cfg = yerd_config::Config::load(&state.config_path).unwrap();
        assert_eq!(cfg.node.default.as_deref(), Some("24.1.0"));
    }

    #[tokio::test]
    async fn clear_node_version_removes_only_that_site() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        let blog = site(&state, tmp.path(), "blog").await;
        let shop = site(&state, tmp.path(), "shop").await;
        set_node_config(&state, None, &[(&blog, "22"), (&shop, "24")]).await;
        assert!(matches!(
            clear_node_version("BLOG", &state).await,
            Response::Ok
        ));
        assert!(matches!(
            clear_node_version("blog", &state).await,
            Response::Ok
        ));
        assert_eq!(
            error_code(&clear_node_version("ghost", &state).await),
            Some(ErrorCode::NotFound)
        );
        let cfg = yerd_config::Config::load(&state.config_path).unwrap();
        assert_eq!(cfg.node.sites.keys().collect::<Vec<_>>(), [&shop]);
    }

    #[tokio::test]
    async fn node_site_names_the_missing_selector() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        release(&state, "20.1.0", None);
        let blog = site(&state, tmp.path(), "blog").await;
        let missing = |r: Response| match r {
            Response::NodeSite { missing, .. } => missing,
            other => panic!("expected NodeSite, got {other:?}"),
        };
        set_node_config(&state, Some("20"), &[]).await;
        assert_eq!(missing(node_site_response("blog", &state).await), None);
        std::fs::write(PathBuf::from(&blog).join(".nvmrc"), "18\n").unwrap();
        assert_eq!(
            missing(node_site_response("blog", &state).await).as_deref(),
            Some("18")
        );
        std::fs::remove_file(PathBuf::from(&blog).join(".nvmrc")).unwrap();
        set_node_config(&state, Some("99"), &[]).await;
        assert_eq!(
            missing(node_site_response("blog", &state).await).as_deref(),
            Some("99")
        );
        assert_eq!(missing(node_site_response("ghost", &state).await), None);
    }

    #[tokio::test]
    async fn node_site_reports_precedence_and_errors_as_data() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        for version in ["20.1.0", "22.9.0", "24.1.0"] {
            release(&state, version, None);
        }
        let blog = site(&state, tmp.path(), "blog").await;
        let root = PathBuf::from(&blog);
        let summary = |r: Response| match r {
            Response::NodeSite {
                preference,
                project_file,
                effective,
                source,
                error,
                ..
            } => (
                preference,
                project_file.map(|f| (f.kind, f.selector)),
                effective,
                source,
                error.is_some(),
            ),
            other => panic!("expected NodeSite, got {other:?}"),
        };
        set_node_config(&state, Some("20"), &[]).await;
        assert_eq!(
            summary(node_site_response("blog", &state).await),
            (
                None,
                None,
                Some("20.1.0".into()),
                Some(NodeSource::Default),
                false
            )
        );
        set_node_config(&state, Some("20"), &[(&blog, "24.1.0")]).await;
        assert_eq!(
            summary(node_site_response("blog", &state).await),
            (
                Some("24.1.0".into()),
                None,
                Some("24.1.0".into()),
                Some(NodeSource::Site),
                false
            )
        );
        std::fs::write(root.join(".node-version"), "22").unwrap();
        assert_eq!(
            summary(node_site_response("blog", &state).await),
            (
                Some("24.1.0".into()),
                Some((NodeSource::NodeVersion, "22".into())),
                Some("22.9.0".into()),
                Some(NodeSource::NodeVersion),
                false
            )
        );
        std::fs::write(root.join(".nvmrc"), "20").unwrap();
        assert_eq!(
            summary(node_site_response("blog", &state).await),
            (
                Some("24.1.0".into()),
                Some((NodeSource::Nvmrc, "20".into())),
                Some("20.1.0".into()),
                Some(NodeSource::Nvmrc),
                false
            )
        );
        std::fs::write(root.join(".nvmrc"), "18").unwrap();
        assert_eq!(
            summary(node_site_response("blog", &state).await),
            (
                Some("24.1.0".into()),
                Some((NodeSource::Nvmrc, "18".into())),
                None,
                None,
                true
            )
        );
        assert_eq!(
            summary(node_site_response("ghost", &state).await),
            (None, None, None, None, true)
        );
    }

    #[tokio::test]
    async fn release_notes_dispatch_serves_fresh_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        let entry = yerd_ipc::ReleaseNotesEntry {
            version: "2.0.4".into(),
            tag: "v2.0.4".into(),
            published_at: "2026-07-25T15:49:58Z".into(),
            prerelease: false,
            summary: "Thing".into(),
            features: vec![],
            fixes: vec![],
            others: vec![],
        };
        *state.release_notes_cache.write().await =
            Some((std::time::Instant::now(), (None, vec![entry.clone()])));
        let resp = dispatch(Request::ReleaseNotes, &state).await;
        assert_eq!(
            resp,
            Response::ReleaseNotes {
                current: None,
                earlier: vec![entry],
            }
        );
    }

    #[tokio::test]
    async fn contributors_dispatch_serves_fresh_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        let person = yerd_ipc::Contributor {
            login: "totov".into(),
            avatar_url: "https://a.test/1".into(),
            commits: 12,
        };
        *state.contributors_cache.write().await =
            Some((std::time::Instant::now(), vec![person.clone()]));
        let resp = dispatch(Request::Contributors, &state).await;
        assert_eq!(
            resp,
            Response::Contributors {
                contributors: vec![person],
                total: 1,
            }
        );
    }

    #[tokio::test]
    async fn node_sites_matches_node_site_for_every_site() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        for version in ["20.1.0", "22.9.0"] {
            release(&state, version, None);
        }
        let blog = site(&state, tmp.path(), "blog").await;
        site(&state, tmp.path(), "shop").await;
        let gone = site(&state, tmp.path(), "gone").await;
        std::fs::remove_dir(&gone).unwrap();
        std::fs::write(PathBuf::from(&blog).join(".nvmrc"), "22").unwrap();
        set_node_config(&state, Some("20"), &[]).await;

        let Response::NodeSites { sites } = dispatch(Request::NodeSites, &state).await else {
            panic!("expected NodeSites");
        };
        let names: Vec<&str> = sites.iter().map(|e| e.site.as_str()).collect();
        let router_names: Vec<String> = state
            .router
            .read()
            .await
            .iter()
            .map(|s| s.name().to_owned())
            .collect();
        assert_eq!(names, router_names);
        for entry in &sites {
            let expected = node_site_response(&entry.site, &state).await;
            let actual = Response::NodeSite {
                preference: entry.preference.clone(),
                project_file: entry.project_file.clone(),
                effective: entry.effective.clone(),
                source: entry.source,
                error: entry.error.clone(),
                missing: entry.missing.clone(),
            };
            assert_eq!(actual, expected, "{}", entry.site);
        }
        let by_name = |n: &str| sites.iter().find(|e| e.site == n).unwrap();
        assert_eq!(by_name("blog").effective.as_deref(), Some("22.9.0"));
        assert_eq!(by_name("blog").source, Some(NodeSource::Nvmrc));
        assert_eq!(by_name("shop").effective.as_deref(), Some("20.1.0"));
        assert!(by_name("gone").error.is_some());
    }

    struct NodeIndexDl {
        schedule: Option<&'static str>,
        index: bool,
    }

    #[async_trait::async_trait]
    impl yerd_php::Downloader for NodeIndexDl {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            let fail = || {
                Err(yerd_php::DownloadError::Transport {
                    url: url.to_owned(),
                    reason: "offline".into(),
                })
            };
            if url.ends_with("schedule.json") {
                return self
                    .schedule
                    .map_or_else(fail, |b| Ok(b.as_bytes().to_vec()));
            }
            if !self.index {
                return fail();
            }
            let files = r#"["osx-arm64-tar","osx-x64-tar","linux-x64","linux-arm64"]"#;
            Ok(format!(
                r#"[{{"version":"v24.1.0","lts":"Krypton","files":{files}}},{{"version":"v22.12.0","lts":"Jod","files":{files}}}]"#
            )
            .into_bytes())
        }
    }

    #[tokio::test]
    async fn available_node_with_marks_eol_from_the_optional_schedule() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        let eol = |r: Response| match r {
            Response::AvailableNode { majors } => majors
                .into_iter()
                .map(|m| (m.major, m.eol))
                .collect::<Vec<_>>(),
            other => panic!("expected AvailableNode, got {other:?}"),
        };
        let dl = NodeIndexDl {
            schedule: Some(r#"{"v22":{"end":"2020-01-01"},"v24":{"end":"2999-01-01"}}"#),
            index: true,
        };
        assert_eq!(
            eol(available_node_with(&state, &dl, "2026-11-17").await),
            [(24, Some(false)), (22, Some(true))]
        );
        let dl = NodeIndexDl {
            schedule: None,
            index: true,
        };
        assert_eq!(
            eol(available_node_with(&state, &dl, "2026-11-17").await),
            [(24, None), (22, None)]
        );
        let dl = NodeIndexDl {
            schedule: None,
            index: false,
        };
        assert_eq!(
            error_code(&available_node_with(&state, &dl, "2026-11-17").await),
            Some(ErrorCode::Internal)
        );
    }

    fn listed_updates(r: Response) -> Vec<(String, Option<String>)> {
        match r {
            Response::NodeVersions { installed } => installed
                .into_iter()
                .map(|n| (n.version, n.update))
                .collect(),
            other => panic!("expected NodeVersions, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn node_update_poll_feeds_list_node_and_skips_without_installs() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        let dl = NodeIndexDl {
            schedule: None,
            index: true,
        };
        crate::node_updates::poll_and_refresh(&state, &dl).await;
        assert!(state.node_updates.read().await.is_empty());
        release(&state, "22.9.0", None);
        release(&state, "24.1.0", None);
        crate::node_updates::poll_and_refresh(&state, &dl).await;
        assert_eq!(build_status_report(&state).await.node_updates, 1);
        assert_eq!(
            listed_updates(node_versions_response(&state).await),
            [
                ("24.1.0".to_owned(), None),
                ("22.9.0".to_owned(), Some("22.12.0".to_owned()))
            ]
        );
        let offline = NodeIndexDl {
            schedule: None,
            index: false,
        };
        crate::node_updates::poll_and_refresh(&state, &offline).await;
        assert_eq!(
            state.node_updates.read().await.get(&22).map(String::as_str),
            Some("22.12.0")
        );
        release(&state, "22.12.0", None);
        reconcile_tool_shims_now(&state).await;
        assert_eq!(build_status_report(&state).await.node_updates, 0);
    }

    #[tokio::test]
    async fn available_node_refreshes_the_update_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        release(&state, "22.9.0", None);
        let dl = NodeIndexDl {
            schedule: None,
            index: true,
        };
        assert!(matches!(
            available_node_with(&state, &dl, "2026-11-17").await,
            Response::AvailableNode { .. }
        ));
        assert_eq!(
            listed_updates(node_versions_response(&state).await),
            [("22.9.0".to_owned(), Some("22.12.0".to_owned()))]
        );
    }

    #[tokio::test]
    async fn after_node_install_moves_only_an_older_patch_default() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        release(&state, "22.9.0", None);
        release(&state, "22.12.0", None);
        for (default, had_default, update, expected) in [
            (Some("22.9.0"), true, true, Some("22.12.0")),
            (Some("22"), true, true, Some("22")),
            (Some("22.9.0"), true, false, Some("22.9.0")),
            (None, false, false, Some("22.12.0")),
        ] {
            set_node_config(&state, default, &[("/srv/a", "22.9.0")]).await;
            assert!(matches!(
                after_node_install(&state, "22.12.0", had_default, update).await,
                Response::Ok
            ));
            let cfg = state.config.lock().await;
            assert_eq!(
                cfg.node.default.as_deref(),
                expected,
                "{default:?} {update}"
            );
            assert_eq!(
                cfg.node.sites.get("/srv/a").map(String::as_str),
                Some("22.9.0")
            );
        }
        set_node_config(&state, None, &[]).await;
        let legacy = state.dirs.data.join("tools/node");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(".default"), "22.9.0").unwrap();
        assert!(matches!(
            after_node_install(&state, "22.12.0", true, true).await,
            Response::Ok
        ));
        let cfg = yerd_config::Config::load(&state.config_path).unwrap();
        assert_eq!(cfg.node.default.as_deref(), Some("22.12.0"));
    }
}
