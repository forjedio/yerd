//! Service supervision wiring: the daemon's `ServiceManager` type, the IPC
//! handlers for the service requests, the `StatusReport.services` builder, and
//! background auto-start.
//!
//! Lock discipline mirrors the PHP path: the slow `ensure`/download work runs
//! **without** the config lock held, and the config lock and the
//! service-manager lock are never held simultaneously across an `.await`.

use std::sync::Arc;

use tokio::sync::watch;
use yerd_config::ServiceInstance;
use yerd_ipc::{ErrorCode, Response, ServiceAvailability, ServiceRunState, ServiceStatus};
use yerd_services::{
    available_versions, current_os_arch, listing_url, version as svc_version, Service,
    ServiceError, ServiceManager, ServiceProbes, ServiceRunState as MgrRunState, ServiceVersion,
};
use yerd_supervise::{Downloader, SystemClock, TokioProcessSpawner};

use crate::service_install;
use crate::state::DaemonState;

/// Concrete `ServiceManager` shape the daemon uses. [`ServiceProbes`] dispatches
/// readiness checks to the right per-engine protocol probe (Redis / MySQL /
/// MariaDB / Postgres).
pub type DaemonServiceManager = ServiceManager<TokioProcessSpawner, SystemClock, ServiceProbes>;

/// Build the daemon's service manager.
#[must_use]
pub fn new_manager(dirs: yerd_platform::PlatformDirs) -> DaemonServiceManager {
    ServiceManager::new(
        TokioProcessSpawner,
        SystemClock,
        ServiceProbes::new(),
        dirs,
        yerd_platform::ActivePortBinder::new(),
    )
}

// ── handlers ────────────────────────────────────────────────────────────────

/// `list services` - every manageable engine with its live status.
pub async fn list_services(state: &DaemonState) -> Response {
    Response::Services {
        services: service_statuses(state).await,
    }
}

/// `available services` - installable vs installed versions per engine. Fetches
/// yerd's services listing on demand; a transport failure is the only error.
pub async fn available_services(state: &DaemonState, dl: &dyn Downloader) -> Response {
    let (os, arch) = match current_os_arch() {
        Ok(p) => p,
        Err(e) => return service_error_response(&e),
    };
    let listing = match dl.download(&listing_url()).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => {
            return Response::Error {
                code: ErrorCode::Internal,
                message: format!("couldn't reach the services distribution: {e}"),
            }
        }
    };
    let services = Service::ALL
        .into_iter()
        .map(|svc| ServiceAvailability {
            service: svc.id().to_string(),
            available: available_versions(&listing, svc, os, arch)
                .iter()
                .map(ToString::to_string)
                .collect(),
            installed: installed_versions(svc, &state.dirs)
                .iter()
                .map(ToString::to_string)
                .collect(),
        })
        .collect();
    Response::AvailableServices { services }
}

/// `install service <svc> <version>` - download + unpack (no config lock held),
/// then start it. Installing a service is taken as intent to run it, so a fresh
/// install comes up immediately and - like every installed engine - survives
/// daemon restarts (see [`auto_start_installed`]). `enabled` is still set for the
/// status record, but no longer gates boot auto-start.
pub async fn install_service(
    service_id: &str,
    version: &str,
    state: &DaemonState,
    dl: &dyn Downloader,
) -> Response {
    let Some(service) = Service::from_id(service_id) else {
        return unknown_service(service_id);
    };
    let version: ServiceVersion = match version.parse() {
        Ok(v) => v,
        Err(e) => return service_error_response(&e),
    };
    if let Err(e) = service_install::install(service, &version, &state.dirs, dl).await {
        return service_error_response(&e);
    }

    let port = {
        let cfg = state.config.lock().await;
        cfg.services
            .instances
            .get(service.id())
            .and_then(|i| i.port)
            .unwrap_or(service.default_port())
    };
    match ensure_and_persist(state, service, version, port).await {
        Ok(()) => Response::Ok,
        Err(resp) => resp,
    }
}

/// Ensure `service` is running at `version`/`port`, then persist it as the
/// selected instance (`enabled`/`version`/`port`) in config. Shared by
/// [`install_service`]/[`start_service`]'s own handlers and by any other
/// in-daemon caller that installs+starts a service inline as part of a larger
/// job (e.g. the WordPress create-site job's database-provisioning phase) -
/// callers must go through this rather than only `ServiceManager::ensure`, or
/// the engine ends up running but unrecorded, and `ListServices`/boot
/// auto-start disagree with reality.
pub(crate) async fn ensure_and_persist(
    state: &DaemonState,
    service: Service,
    version: ServiceVersion,
    port: u16,
) -> Result<(), Response> {
    {
        let mut mgr = state.service_manager.lock().await;
        mgr.ensure(service, version.clone(), port)
            .await
            .map_err(|e| service_error_response(&e))?;
    }
    persist_instance(state, service, |inst| {
        inst.enabled = true;
        inst.version = Some(version.to_string());
        inst.port = Some(port);
    })
    .await
    .map(|_| ())
}

/// `change-version <svc> <new>` - switch the engine's single installed version.
/// Installs the new version, restarts the instance onto it, then removes the
/// previously-installed version(s). The datadir is retained (it's shared per
/// engine / per major), so this is safe for SQL engines in later phases.
pub async fn change_service_version(
    service_id: &str,
    version: &str,
    state: &DaemonState,
    dl: &dyn Downloader,
) -> Response {
    let Some(service) = Service::from_id(service_id) else {
        return unknown_service(service_id);
    };
    let new_version: ServiceVersion = match version.parse() {
        Ok(v) => v,
        Err(e) => return service_error_response(&e),
    };

    let superseded: Vec<ServiceVersion> = installed_versions(service, &state.dirs)
        .into_iter()
        .filter(|v| v != &new_version)
        .collect();

    if let Err(e) = service_install::install(service, &new_version, &state.dirs, dl).await {
        return service_error_response(&e);
    }

    let port = {
        let cfg = state.config.lock().await;
        cfg.services
            .instances
            .get(service.id())
            .and_then(|i| i.port)
            .unwrap_or(service.default_port())
    };
    let outcome = {
        let mut mgr = state.service_manager.lock().await;
        mgr.restart(service, new_version.clone(), port).await
    };
    if let Err(e) = outcome {
        return service_error_response(&e);
    }

    if let Err(resp) = persist_instance(state, service, |inst| {
        inst.enabled = true;
        inst.version = Some(new_version.to_string());
        inst.port = Some(port);
    })
    .await
    {
        return resp;
    }
    for old in superseded {
        if let Err(e) = service_install::uninstall(service, &old, &state.dirs, false) {
            tracing::warn!(
                service = %service,
                version = %old,
                error = %e,
                "couldn't remove superseded service version"
            );
        }
    }
    Response::Ok
}

/// `uninstall service <svc> <version> [--purge]`.
pub async fn uninstall_service(
    service_id: &str,
    version: &str,
    purge: bool,
    state: &DaemonState,
) -> Response {
    let Some(service) = Service::from_id(service_id) else {
        return unknown_service(service_id);
    };
    let version: ServiceVersion = match version.parse() {
        Ok(v) => v,
        Err(e) => return service_error_response(&e),
    };
    let _ = state.service_manager.lock().await.stop(service).await;
    match service_install::uninstall(service, &version, &state.dirs, purge) {
        Ok(retained) => {
            if let Some(path) = retained {
                tracing::info!(
                    service = %service,
                    datadir = %path.display(),
                    "uninstalled service; datadir retained (use --purge to delete)"
                );
            }
            Response::Ok
        }
        Err(e) => service_error_response(&e),
    }
}

/// `start service <svc>` - ensure it's running, enable auto-start, persist config.
pub async fn start_service(service_id: &str, state: &DaemonState) -> Response {
    let Some(service) = Service::from_id(service_id) else {
        return unknown_service(service_id);
    };
    let (configured_version, port) = {
        let cfg = state.config.lock().await;
        let inst = cfg.services.instances.get(service.id());
        (
            inst.and_then(|i| i.version.clone()),
            inst.and_then(|i| i.port).unwrap_or(service.default_port()),
        )
    };
    let version = match resolve_version(service, configured_version.as_deref(), &state.dirs) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    match ensure_and_persist(state, service, version, port).await {
        Ok(()) => Response::Ok,
        Err(resp) => resp,
    }
}

/// `stop service <svc>` - stop it and disable auto-start.
pub async fn stop_service(service_id: &str, state: &DaemonState) -> Response {
    let Some(service) = Service::from_id(service_id) else {
        return unknown_service(service_id);
    };
    {
        let mut mgr = state.service_manager.lock().await;
        if let Err(e) = mgr.stop(service).await {
            return service_error_response(&e);
        }
    }
    persist_instance(state, service, |inst| inst.enabled = false)
        .await
        .unwrap_or_else(|resp| resp)
}

/// `restart service <svc>` - stop + ensure with the configured/selected version.
pub async fn restart_service(service_id: &str, state: &DaemonState) -> Response {
    let Some(service) = Service::from_id(service_id) else {
        return unknown_service(service_id);
    };
    let (configured_version, port) = {
        let cfg = state.config.lock().await;
        let inst = cfg.services.instances.get(service.id());
        (
            inst.and_then(|i| i.version.clone()),
            inst.and_then(|i| i.port).unwrap_or(service.default_port()),
        )
    };
    let version = match resolve_version(service, configured_version.as_deref(), &state.dirs) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let outcome = {
        let mut mgr = state.service_manager.lock().await;
        mgr.restart(service, version, port).await
    };
    match outcome {
        Ok(_) => Response::Ok,
        Err(e) => service_error_response(&e),
    }
}

/// `set-port <svc> <port>` - persist the port (takes effect on next start/restart).
pub async fn set_service_port(service_id: &str, port: u16, state: &DaemonState) -> Response {
    let Some(service) = Service::from_id(service_id) else {
        return unknown_service(service_id);
    };
    persist_instance(state, service, |inst| inst.port = Some(port))
        .await
        .unwrap_or_else(|resp| resp)
}

/// `service logs <svc>` - the last `lines` lines of the engine's log file.
pub fn service_logs(service_id: &str, lines: u32, state: &DaemonState) -> Response {
    let Some(service) = Service::from_id(service_id) else {
        return unknown_service(service_id);
    };
    let path = svc_version::log_path(&state.dirs, service);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Response::Error {
                code: ErrorCode::Internal,
                message: format!("read {} log: {e}", service.id()),
            }
        }
    };
    let want = lines as usize;
    let all: Vec<&str> = content.lines().collect();
    let start = all.len().saturating_sub(want);
    let tail = all
        .get(start..)
        .unwrap_or(&[])
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    Response::ServiceLogs { lines: tail }
}

// ── status + auto-start ───────────────────────────────────────────────────

/// Build the per-service status list for `ListServices` / `StatusReport`.
pub async fn service_statuses(state: &DaemonState) -> Vec<ServiceStatus> {
    let snapshots = {
        let mut mgr = state.service_manager.lock().await;
        mgr.snapshots()
    };
    let instances = {
        let cfg = state.config.lock().await;
        cfg.services.instances.clone()
    };

    Service::ALL
        .into_iter()
        .map(|svc| {
            let inst = instances.get(svc.id());
            let snap = snapshots.iter().find(|s| s.service == svc);
            let (run_state, pid, listen) = match snap {
                Some(s) => (
                    map_run_state(s.state),
                    s.pid,
                    s.listen.as_ref().map(ToString::to_string),
                ),
                None => (ServiceRunState::Stopped, None, None),
            };
            ServiceStatus {
                service: svc.id().to_string(),
                display_name: svc.display_name().to_string(),
                installed_versions: installed_versions(svc, &state.dirs)
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                selected_version: inst.and_then(|i| i.version.clone()),
                state: run_state,
                pid,
                listen,
                port: inst.and_then(|i| i.port).unwrap_or(svc.default_port()),
                enabled: inst.is_some_and(|i| i.enabled),
                supports_databases: matches!(svc.kind(), yerd_services::ServiceKind::Database),
            }
        })
        .collect()
}

/// Auto-start every *installed* service at daemon startup. Runs as a background
/// task so a slow/failing DB cold-boot never blocks the proxy/DNS listeners.
///
/// Policy: any engine with an installed version is brought up on boot,
/// regardless of the persisted `enabled` flag - installing a service is taken as
/// intent to run it. A `Stop` still stops the engine for the current session,
/// but it returns on the next daemon start; to keep one off for good, uninstall
/// it.
///
/// Shutdown-aware: an instance torn down shortly after booting (the
/// upgrade-restart thrash) bails before spawning DB engines, so the 10s DB
/// `stop_grace` never holds the instance lock and serialises the next relaunch.
pub async fn auto_start_installed(state: Arc<DaemonState>) {
    let installed: Vec<Service> = Service::ALL
        .into_iter()
        .filter(|svc| !installed_versions(*svc, &state.dirs).is_empty())
        .collect();
    let mut shutdown = state.shutdown_tx.subscribe();
    run_auto_start(installed, &mut shutdown, |service| {
        let state = state.clone();
        async move { list_services_start_one(service, &state).await }
    })
    .await;
}

/// Start each installed service in order, stopping the moment `shutdown` trips.
/// Extracted from [`auto_start_installed`] so both the already-shutting-down and
/// the mid-loop-abort branches are unit-testable with a fake `start_one`. The
/// `biased` select checks shutdown first each iteration; a SIGTERM landing
/// mid-start cancels the in-flight future, whose not-yet-tracked child the
/// spawner's `kill_on_drop` then reaps.
async fn run_auto_start<F, Fut>(
    installed: Vec<Service>,
    shutdown: &mut watch::Receiver<bool>,
    mut start_one: F,
) where
    F: FnMut(Service) -> Fut,
    Fut: std::future::Future<Output = Result<(), ServiceError>>,
{
    if *shutdown.borrow() {
        return;
    }
    for service in installed {
        tokio::select! {
            biased;
            _ = shutdown.changed() => return,
            res = start_one(service) => match res {
                Ok(()) => tracing::info!(service = %service, "auto-started service"),
                Err(e) => tracing::warn!(service = %service, error = %e, "service auto-start failed"),
            },
        }
    }
}

/// Ensure one installed service is running (used by auto-start). Returns the
/// supervisor error so the caller can log it.
async fn list_services_start_one(
    service: Service,
    state: &DaemonState,
) -> Result<(), ServiceError> {
    let (configured_version, port) = {
        let cfg = state.config.lock().await;
        let inst = cfg.services.instances.get(service.id());
        (
            inst.and_then(|i| i.version.clone()),
            inst.and_then(|i| i.port).unwrap_or(service.default_port()),
        )
    };
    let version = match configured_version {
        Some(v) => v.parse::<ServiceVersion>()?,
        None => {
            installed_versions(service, &state.dirs)
                .pop()
                .ok_or(ServiceError::Unsupported {
                    service,
                    detail: "no installed version to auto-start".to_owned(),
                })?
        }
    };
    let mut mgr = state.service_manager.lock().await;
    mgr.ensure(service, version, port).await.map(|_| ())
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Installed versions of `service`, ascending.
fn installed_versions(service: Service, dirs: &yerd_platform::PlatformDirs) -> Vec<ServiceVersion> {
    svc_version::discover_installed(dirs)
        .ok()
        .and_then(|mut m| m.remove(&service))
        .unwrap_or_default()
}

/// Resolve the version to run: the configured one if installed, else the latest
/// installed; error if nothing is installed.
pub(crate) fn resolve_version(
    service: Service,
    configured: Option<&str>,
    dirs: &yerd_platform::PlatformDirs,
) -> Result<ServiceVersion, Response> {
    let mut installed = installed_versions(service, dirs);
    if let Some(c) = configured {
        if let Ok(v) = c.parse::<ServiceVersion>() {
            if installed.contains(&v) {
                return Ok(v);
            }
        }
    }
    installed.pop().ok_or_else(|| Response::Error {
        code: ErrorCode::NotFound,
        message: format!(
            "no {} version installed - run `yerd service install {}` first",
            service.display_name(),
            service.id()
        ),
    })
}

/// Apply a mutation to a service's config instance, validate, and persist.
async fn persist_instance(
    state: &DaemonState,
    service: Service,
    f: impl FnOnce(&mut ServiceInstance),
) -> Result<Response, Response> {
    let mut cfg = state.config.lock().await;
    let inst = cfg
        .services
        .instances
        .entry(service.id().to_string())
        .or_default();
    f(inst);
    if let Err(e) = cfg.validate() {
        return Err(Response::Error {
            code: ErrorCode::Internal,
            message: format!("config validation failed: {e}"),
        });
    }
    if let Err(e) = cfg.save(&state.config_path) {
        return Err(Response::Error {
            code: ErrorCode::Internal,
            message: format!("persist config: {e}"),
        });
    }
    Ok(Response::Ok)
}

fn map_run_state(s: MgrRunState) -> ServiceRunState {
    match s {
        MgrRunState::Running => ServiceRunState::Running,
        MgrRunState::Failed => ServiceRunState::Failed,
    }
}

fn unknown_service(id: &str) -> Response {
    Response::Error {
        code: ErrorCode::NotFound,
        message: format!("unknown service {id:?}"),
    }
}

fn service_error_response(e: &ServiceError) -> Response {
    Response::Error {
        code: service_error_code(e),
        message: e.to_string(),
    }
}

fn service_error_code(e: &ServiceError) -> ErrorCode {
    match e {
        ServiceError::PortInUse { .. } => ErrorCode::PortInUse,
        ServiceError::VersionNotInstalled { .. } => ErrorCode::NotFound,
        ServiceError::VersionUnavailable { .. }
        | ServiceError::UnsupportedPlatform { .. }
        | ServiceError::Unsupported { .. } => ErrorCode::InvalidPath,
        _ => ErrorCode::Internal,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::path::Path;
    use tokio::sync::{Mutex, RwLock};
    use yerd_core::{RouterConfig, SiteRouter, Tld};
    use yerd_platform::PlatformDirs;

    fn dirs_in(tmp: &std::path::Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    /// Copied verbatim from `ipc_server`'s test module (its `state_in` is private
    /// to that module). Builds a fully-wired `DaemonState` rooted at `tmp` with no
    /// services installed and a default config.
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
            php_ca_bundle: None,
            php_updates: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            yerd_update: tokio::sync::RwLock::new(Vec::new()),
            update_snapshot: tokio::sync::RwLock::new(None),
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
            redirect_https_port: std::sync::Arc::new(std::sync::atomic::AtomicU16::new(8443)),
            web_unbound: None,
            dns_unbound: None,
            boot_id: 1,
            started_at: std::time::Instant::now(),
            shutdown_tx: tokio::sync::watch::channel(false).0,
            restart_requested: std::sync::atomic::AtomicBool::new(false),
            detect_cache: std::sync::Arc::new(crate::detect_cache::DetectCache::new()),
            watch_dirty: tokio::sync::Notify::new(),
            dumps: std::sync::Arc::new(crate::dump_server::DumpStore::new()),
            shim_reconcile: tokio::sync::Mutex::new(()),
            tunnel_manager: std::sync::Arc::new(tokio::sync::Mutex::new(
                crate::tunnel::new_manager(),
            )),
            cloudflared_resolution: tokio::sync::RwLock::new(None),
            tool_mutate: tokio::sync::Mutex::new(()),
            tunnel_mutate: tokio::sync::Mutex::new(()),
            php_mutate: tokio::sync::Mutex::new(()),
            jobs: crate::jobs::JobRegistry::default(),
            reserved_names: tokio::sync::Mutex::new(std::collections::HashSet::new()),
            wordpress_versions: tokio::sync::RwLock::new(None),
        }
    }

    /// A `Downloader` that always reports a transport failure, or yields a fixed
    /// listing body - enough to drive `available_services`' two arms without IO.
    struct FakeDownloader {
        body: Option<Vec<u8>>,
    }

    #[async_trait::async_trait]
    impl Downloader for FakeDownloader {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_supervise::DownloadError> {
            match &self.body {
                Some(b) => Ok(b.clone()),
                None => Err(yerd_supervise::DownloadError::Transport {
                    url: url.to_owned(),
                    reason: "offline".into(),
                }),
            }
        }
    }

    /// Fabricate an "installed" service version by laying down its server binary
    /// at the layout `svc_version::discover_installed` scans for.
    fn install_fake(dirs: &PlatformDirs, service: Service, version: &str) {
        let ver: ServiceVersion = version.parse().unwrap();
        let bin = svc_version::install_dir(dirs, service, &ver).join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(service.server_binary()), b"#!fake").unwrap();
    }

    fn ver(s: &str) -> ServiceVersion {
        s.parse().unwrap()
    }

    #[test]
    fn map_run_state_maps_both_variants() {
        assert_eq!(
            map_run_state(MgrRunState::Running),
            ServiceRunState::Running
        );
        assert_eq!(map_run_state(MgrRunState::Failed), ServiceRunState::Failed);
    }

    #[test]
    fn service_error_code_classifies_variants() {
        let svc = Service::MySql;
        assert_eq!(
            service_error_code(&ServiceError::PortInUse {
                service: svc,
                port: 3306
            }),
            ErrorCode::PortInUse
        );
        assert_eq!(
            service_error_code(&ServiceError::VersionNotInstalled {
                service: svc,
                version: ver("8.4")
            }),
            ErrorCode::NotFound
        );
        assert_eq!(
            service_error_code(&ServiceError::VersionUnavailable {
                service: svc,
                version: ver("8.4")
            }),
            ErrorCode::InvalidPath
        );
        assert_eq!(
            service_error_code(&ServiceError::UnsupportedPlatform {
                detail: "no builds".to_owned()
            }),
            ErrorCode::InvalidPath
        );
        assert_eq!(
            service_error_code(&ServiceError::Unsupported {
                service: svc,
                detail: "nope".to_owned()
            }),
            ErrorCode::InvalidPath
        );
        assert_eq!(
            service_error_code(&ServiceError::ListingParse {
                detail: "bad".to_owned()
            }),
            ErrorCode::Internal
        );
    }

    #[test]
    fn unknown_service_is_not_found() {
        match unknown_service("nope") {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::NotFound);
                assert!(message.contains("nope"));
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn service_error_response_carries_message_and_code() {
        let e = ServiceError::PortInUse {
            service: Service::Postgres,
            port: 5432,
        };
        match service_error_response(&e) {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::PortInUse);
                assert_eq!(message, e.to_string());
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn installed_versions_empty_when_nothing_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        assert!(installed_versions(Service::MySql, &dirs).is_empty());
    }

    #[test]
    fn installed_versions_discovers_laid_down_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        install_fake(&dirs, Service::MariaDb, "11.4");
        let found = installed_versions(Service::MariaDb, &dirs);
        assert_eq!(found, vec![ver("11.4")]);
        assert!(installed_versions(Service::MySql, &dirs).is_empty());
    }

    #[test]
    fn resolve_version_errors_when_nothing_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        match resolve_version(Service::Redis, None, &dirs) {
            Err(Response::Error { code, message }) => {
                assert_eq!(code, ErrorCode::NotFound);
                assert!(message.contains("redis"));
            }
            other => panic!("expected NotFound error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_version_prefers_configured_when_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        install_fake(&dirs, Service::Postgres, "16.2");
        install_fake(&dirs, Service::Postgres, "17.0");
        assert_eq!(
            resolve_version(Service::Postgres, Some("16.2"), &dirs).unwrap(),
            ver("16.2")
        );
    }

    #[test]
    fn resolve_version_falls_back_to_latest_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        install_fake(&dirs, Service::Postgres, "16.2");
        install_fake(&dirs, Service::Postgres, "17.0");
        assert_eq!(
            resolve_version(Service::Postgres, Some("99.0"), &dirs).unwrap(),
            ver("17.0")
        );
        assert_eq!(
            resolve_version(Service::Postgres, None, &dirs).unwrap(),
            ver("17.0")
        );
    }

    #[test]
    fn new_manager_constructs_without_io() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let _mgr = new_manager(dirs);
    }

    /// Pull `(code, message)` out of a `Response::Error`, panicking otherwise.
    fn err_parts(r: Response) -> (ErrorCode, String) {
        match r {
            Response::Error { code, message } => (code, message),
            other => panic!("expected Response::Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn service_statuses_lists_all_engines_stopped_when_none_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let statuses = service_statuses(&state).await;
        assert_eq!(statuses.len(), Service::ALL.len());
        for s in &statuses {
            assert_eq!(s.state, ServiceRunState::Stopped);
            assert!(s.pid.is_none());
            assert!(s.listen.is_none());
            assert!(s.installed_versions.is_empty());
            assert!(s.selected_version.is_none());
            assert!(!s.enabled);
        }
        let mysql = statuses.iter().find(|s| s.service == "mysql").unwrap();
        assert!(mysql.supports_databases);
        let redis = statuses.iter().find(|s| s.service == "redis").unwrap();
        assert!(!redis.supports_databases);
    }

    #[tokio::test]
    async fn list_services_wraps_statuses() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match list_services(&state).await {
            Response::Services { services } => {
                assert_eq!(services.len(), Service::ALL.len());
            }
            other => panic!("expected Services, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn available_services_reports_transport_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let dl = FakeDownloader { body: None };
        let (code, msg) = err_parts(available_services(&state, &dl).await);
        assert_eq!(code, ErrorCode::Internal);
        assert!(msg.contains("services distribution"));
    }

    #[tokio::test]
    async fn available_services_success_lists_every_engine_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let dl = FakeDownloader {
            body: Some(b"{\"services\":{}}".to_vec()),
        };
        match available_services(&state, &dl).await {
            Response::AvailableServices { services } => {
                assert_eq!(services.len(), Service::ALL.len());
                for s in &services {
                    assert!(s.available.is_empty());
                    assert!(s.installed.is_empty());
                }
            }
            other => panic!("expected AvailableServices, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_service_id_is_not_found_across_handlers() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let dl = FakeDownloader { body: None };
        let cases = vec![
            install_service("nope", "1.0", &state, &dl).await,
            change_service_version("nope", "1.0", &state, &dl).await,
            uninstall_service("nope", "1.0", false, &state).await,
            start_service("nope", &state).await,
            stop_service("nope", &state).await,
            restart_service("nope", &state).await,
            set_service_port("nope", 1234, &state).await,
            service_logs("nope", 10, &state),
        ];
        for r in cases {
            assert_eq!(err_parts(r).0, ErrorCode::NotFound);
        }
    }

    #[tokio::test]
    async fn install_and_change_reject_bad_version_string() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let dl = FakeDownloader { body: None };
        let (code, _) = err_parts(install_service("mysql", "not-a-version!!", &state, &dl).await);
        assert_eq!(code, ErrorCode::InvalidPath);
        let (code, _) =
            err_parts(change_service_version("mysql", "not-a-version!!", &state, &dl).await);
        assert_eq!(code, ErrorCode::InvalidPath);
        let (code, _) = err_parts(uninstall_service("mysql", "bad!", false, &state).await);
        assert_eq!(code, ErrorCode::InvalidPath);
    }

    #[tokio::test]
    async fn start_and_restart_error_when_nothing_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let (code, msg) = err_parts(start_service("postgres", &state).await);
        assert_eq!(code, ErrorCode::NotFound);
        assert!(msg.contains("install"));
        assert_eq!(
            err_parts(restart_service("postgres", &state).await).0,
            ErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn stop_service_succeeds_and_persists_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert!(matches!(stop_service("redis", &state).await, Response::Ok));
        let cfg = state.config.lock().await;
        let inst = cfg.services.instances.get("redis").unwrap();
        assert!(!inst.enabled);
    }

    #[tokio::test]
    async fn set_service_port_persists_port() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert!(matches!(
            set_service_port("mysql", 13306, &state).await,
            Response::Ok
        ));
        let cfg = state.config.lock().await;
        assert_eq!(
            cfg.services.instances.get("mysql").unwrap().port,
            Some(13306)
        );
    }

    #[tokio::test]
    async fn service_logs_empty_when_no_log_file() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        match service_logs("mysql", 50, &state) {
            Response::ServiceLogs { lines } => assert!(lines.is_empty()),
            other => panic!("expected ServiceLogs, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn service_logs_tails_last_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let path = svc_version::log_path(&state.dirs, Service::MySql);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"l1\nl2\nl3\nl4\nl5\n").unwrap();
        match service_logs("mysql", 2, &state) {
            Response::ServiceLogs { lines } => assert_eq!(lines, vec!["l4", "l5"]),
            other => panic!("expected ServiceLogs, got {other:?}"),
        }
        match service_logs("mysql", 100, &state) {
            Response::ServiceLogs { lines } => assert_eq!(lines.len(), 5),
            other => panic!("expected ServiceLogs, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn auto_start_installed_is_noop_with_nothing_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = std::sync::Arc::new(state_in(tmp.path()));
        auto_start_installed(state).await;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod auto_start_tests {
    use std::sync::Mutex;

    use super::{run_auto_start, watch, Arc, Service, ServiceError};

    fn two_services() -> Vec<Service> {
        Service::ALL.into_iter().take(2).collect()
    }

    #[tokio::test]
    async fn skips_everything_when_shutdown_already_requested() {
        let (_tx, mut rx) = watch::channel(true);
        let started: Arc<Mutex<Vec<Service>>> = Arc::new(Mutex::new(Vec::new()));
        let rec = started.clone();
        run_auto_start(two_services(), &mut rx, move |svc| {
            let rec = rec.clone();
            async move {
                rec.lock().unwrap().push(svc);
                Ok::<(), ServiceError>(())
            }
        })
        .await;
        assert!(started.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn stops_after_shutdown_trips_mid_loop() {
        let (tx, mut rx) = watch::channel(false);
        let tx = Arc::new(tx);
        let started: Arc<Mutex<Vec<Service>>> = Arc::new(Mutex::new(Vec::new()));
        let rec = started.clone();
        run_auto_start(two_services(), &mut rx, move |svc| {
            let rec = rec.clone();
            let tx = tx.clone();
            async move {
                rec.lock().unwrap().push(svc);
                let _ = tx.send(true);
                Ok::<(), ServiceError>(())
            }
        })
        .await;
        let started = started.lock().unwrap();
        assert_eq!(started.len(), 1, "only the first service should start");
        assert_eq!(started[0], Service::ALL[0]);
    }

    /// Shutdown landing *while* `start_one` is still pending must cancel that
    /// in-flight start (and skip the rest) - the load-bearing behaviour of the
    /// `biased` select. The first start parks on a never-fired gate, so the only
    /// way the spawned task completes is the shutdown arm cancelling it.
    #[tokio::test]
    async fn shutdown_cancels_in_flight_start_one() {
        use tokio::sync::Notify;
        use tokio::time::{timeout, Duration};

        let (tx, rx) = watch::channel(false);
        let entered = Arc::new(Notify::new());
        let gate = Arc::new(Notify::new());
        let started: Arc<Mutex<Vec<Service>>> = Arc::new(Mutex::new(Vec::new()));
        let completed: Arc<Mutex<Vec<Service>>> = Arc::new(Mutex::new(Vec::new()));

        let entered_task = entered.clone();
        let gate_task = gate.clone();
        let started_task = started.clone();
        let completed_task = completed.clone();
        let handle = tokio::spawn(async move {
            let mut rx = rx;
            run_auto_start(two_services(), &mut rx, move |svc| {
                let entered = entered_task.clone();
                let gate = gate_task.clone();
                let started = started_task.clone();
                let completed = completed_task.clone();
                async move {
                    started.lock().unwrap().push(svc);
                    entered.notify_one();
                    gate.notified().await;
                    completed.lock().unwrap().push(svc);
                    Ok::<(), ServiceError>(())
                }
            })
            .await;
        });

        timeout(Duration::from_secs(1), entered.notified())
            .await
            .expect("first start_one should be entered");
        let _ = tx.send(true);

        timeout(Duration::from_secs(1), handle)
            .await
            .expect("biased shutdown must cancel the in-flight start_one")
            .unwrap();

        assert!(
            completed.lock().unwrap().is_empty(),
            "the cancelled start must not run past its await"
        );
        let started = started.lock().unwrap();
        assert_eq!(started.len(), 1, "only the first service should start");
        assert_eq!(started[0], Service::ALL[0]);
    }
}
