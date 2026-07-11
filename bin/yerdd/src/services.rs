//! Service supervision wiring: the daemon's `ServiceManager` type, the IPC
//! handlers for the service requests, the `StatusReport.services` builder, and
//! background auto-start.
//!
//! Instances are keyed by their *wire id*: a bare type id (`"redis"`) for a
//! single-instance engine, or `"{type}:{site}"` (`"reverb:blog"`) for a per-site
//! app server. All per-type behaviour is dispatched through the
//! [`ServiceRegistry`]; the manager never sees a closed enum.
//!
//! Lock discipline mirrors the PHP path: the slow `ensure`/download work runs
//! **without** the config lock held, and the config lock and the
//! service-manager lock are never held simultaneously across an `.await`.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::watch;
use yerd_config::{Config, ServiceInstance};
use yerd_ipc::{
    AddableServiceType, ErrorCode, Request, Response, ServiceAvailability, ServiceRunState,
    ServiceStatus,
};
use yerd_platform::{ActivePortBinder, PlatformDirs, PortBinder};
use yerd_services::{
    available_versions, candidate_ports, current_os_arch, listing_url, version as svc_version,
    Multiplicity, ServiceDefinition, ServiceError, ServiceManager, ServiceProbes, ServiceRegistry,
    ServiceRunState as MgrRunState, ServiceVersion,
};
use yerd_supervise::{Downloader, SystemClock, TokioProcessSpawner};

use crate::service_install;
use crate::state::DaemonState;

/// Concrete `ServiceManager` shape the daemon uses.
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

/// The built-in service-type registry (cheap - a `Vec` of five `Arc`s).
fn registry() -> ServiceRegistry {
    ServiceRegistry::builtin()
}

/// Split an instance wire id into `(type_id, site)`.
fn parse_wire_id(id: &str) -> (String, Option<String>) {
    match id.split_once(':') {
        Some((ty, site)) => (ty.to_owned(), Some(site.to_owned())),
        None => (id.to_owned(), None),
    }
}

/// Format an instance wire id from a type id and optional site.
fn wire_id(type_id: &str, site: Option<&str>) -> String {
    match site {
        Some(s) => format!("{type_id}:{s}"),
        None => type_id.to_owned(),
    }
}

/// Whether a site suffix is a safe DNS-style label: non-empty, <= 63 bytes, no
/// leading/trailing `-`, only `[a-z0-9-]`. Guards against a wire id whose site
/// component carries `..` or a path separator into a derived file path.
fn valid_site_label(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Whether `wire_id` is a well-formed instance id: a known type, and (for a
/// per-site id) a valid site label. Used to reject a malicious or malformed id
/// before it reaches a filesystem path.
fn valid_instance_id(wire_id: &str) -> bool {
    let (type_id, site) = parse_wire_id(wire_id);
    registry().get(&type_id).is_some() && site.as_deref().map_or(true, valid_site_label)
}

// ── handlers ────────────────────────────────────────────────────────────────

/// `list services` - one row per single-instance type plus one per configured
/// per-site instance.
pub async fn list_services(state: &DaemonState) -> Response {
    Response::Services {
        services: service_statuses(state).await,
    }
}

/// `available services` - installable vs installed versions per *versioned*
/// engine (version-less app servers are excluded; they have no download).
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
    let reg = registry();
    let services = reg
        .iter()
        .filter(|d| d.requires_version())
        .map(|d| ServiceAvailability {
            service: d.id().to_string(),
            available: available_versions(&listing, d.id(), os, arch)
                .iter()
                .map(ToString::to_string)
                .collect(),
            installed: installed_versions(d.id(), &state.dirs)
                .iter()
                .map(ToString::to_string)
                .collect(),
        })
        .collect();
    Response::AvailableServices { services }
}

/// `addable-service-types` - the "Add Service" dialog catalog: per-type
/// multiplicity, install state, versions, and a suggested next-free port.
pub async fn addable_service_types(state: &DaemonState, dl: &dyn Downloader) -> Response {
    let listing = match current_os_arch() {
        Ok(_) => dl
            .download(&listing_url())
            .await
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    let (os, arch) =
        current_os_arch().unwrap_or((yerd_services::Os::Linux, yerd_services::Arch::X86_64));

    let reserved = {
        let cfg = state.config.lock().await;
        reserved_ports(&cfg)
    };

    let reg = registry();
    let types = reg
        .iter()
        .map(|d| {
            let available_versions: Vec<String> = if d.requires_version() {
                available_versions(&listing, d.id(), os, arch)
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            } else {
                Vec::new()
            };
            let already_installed = matches!(d.multiplicity(), Multiplicity::Single)
                && !installed_versions(d.id(), &state.dirs).is_empty();
            let suggested_port =
                pick_free_port(d.default_port(), &reserved).unwrap_or(d.default_port());
            AddableServiceType {
                type_id: d.id().to_string(),
                display_name: d.display_name().to_string(),
                multiplicity: match d.multiplicity() {
                    Multiplicity::Single => "single".to_string(),
                    Multiplicity::PerSite => "per_site".to_string(),
                },
                requires_site: d.requires_site(),
                requires_version: d.requires_version(),
                already_installed,
                available_versions,
                default_port: d.default_port(),
                suggested_port,
            }
        })
        .collect();
    Response::AddableServices { types }
}

/// `install service <svc> <version>` - download + unpack (no config lock held),
/// then start it. Only valid for a versioned single-instance engine.
pub async fn install_service(
    service_id: &str,
    version: &str,
    state: &DaemonState,
    dl: &dyn Downloader,
) -> Response {
    let reg = registry();
    let Some(def) = reg.get(service_id) else {
        return unknown_service(service_id);
    };
    if !def.requires_version() {
        return service_type_mismatch(service_id, "is not installed by version");
    }
    let version: ServiceVersion = match version.parse() {
        Ok(v) => v,
        Err(e) => return service_error_response(&e),
    };
    if let Err(e) =
        service_install::install(def.id(), def.server_binary(), &version, &state.dirs, dl).await
    {
        return service_error_response(&e);
    }

    let port = {
        let cfg = state.config.lock().await;
        cfg.services
            .instances
            .get(def.id())
            .and_then(|i| i.port)
            .unwrap_or(def.default_port())
    };
    match ensure_and_persist(state, &def, def.id(), Some(version), port, None, None).await {
        Ok(()) => Response::Ok,
        Err(resp) => resp,
    }
}

/// Ensure the `wire_id` instance is running, then persist it. Shared by the
/// install/start handlers and by any in-daemon caller that installs+starts a
/// service inline (e.g. the WordPress create-site job's DB provisioning).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn ensure_and_persist(
    state: &DaemonState,
    def: &Arc<dyn ServiceDefinition>,
    wire: &str,
    version: Option<ServiceVersion>,
    port: u16,
    program_override: Option<PathBuf>,
    cwd: Option<PathBuf>,
) -> Result<(), Response> {
    {
        let mut mgr = state.service_manager.lock().await;
        mgr.ensure(
            Arc::clone(def),
            wire,
            version.clone(),
            port,
            program_override,
            cwd,
        )
        .await
        .map_err(|e| service_error_response(&e))?;
    }
    let (type_id, site) = parse_wire_id(wire);
    let vstr = version.as_ref().map(ToString::to_string);
    persist_instance(state, wire, |inst| {
        inst.version = vstr;
        inst.port = Some(port);
        inst.site = site;
        let _ = &type_id;
    })
    .await
    .map(|_| ())
}

/// `change-version <svc> <new>` - switch a versioned engine's installed version.
pub async fn change_service_version(
    service_id: &str,
    version: &str,
    state: &DaemonState,
    dl: &dyn Downloader,
) -> Response {
    let reg = registry();
    let Some(def) = reg.get(service_id) else {
        return unknown_service(service_id);
    };
    if !def.requires_version() {
        return service_type_mismatch(service_id, "has no versions to change");
    }
    let new_version: ServiceVersion = match version.parse() {
        Ok(v) => v,
        Err(e) => return service_error_response(&e),
    };

    let superseded: Vec<ServiceVersion> = installed_versions(def.id(), &state.dirs)
        .into_iter()
        .filter(|v| v != &new_version)
        .collect();

    if let Err(e) =
        service_install::install(def.id(), def.server_binary(), &new_version, &state.dirs, dl).await
    {
        return service_error_response(&e);
    }

    let port = {
        let cfg = state.config.lock().await;
        cfg.services
            .instances
            .get(def.id())
            .and_then(|i| i.port)
            .unwrap_or(def.default_port())
    };
    let outcome = {
        let mut mgr = state.service_manager.lock().await;
        mgr.restart(
            Arc::clone(&def),
            def.id(),
            Some(new_version.clone()),
            port,
            None,
            None,
        )
        .await
    };
    if let Err(e) = outcome {
        return service_error_response(&e);
    }

    if let Err(resp) = persist_instance(state, def.id(), |inst| {
        inst.version = Some(new_version.to_string());
        inst.port = Some(port);
    })
    .await
    {
        return resp;
    }
    for old in superseded {
        if let Err(e) = service_install::uninstall(
            def.id(),
            def.datadir_pinned_to_major(),
            &old,
            &state.dirs,
            false,
        ) {
            tracing::warn!(service = def.id(), version = %old, error = %e,
                "couldn't remove superseded service version");
        }
    }
    Response::Ok
}

/// `uninstall service <svc> <version> [--purge]` - versioned engines.
pub async fn uninstall_service(
    service_id: &str,
    version: &str,
    purge: bool,
    state: &DaemonState,
) -> Response {
    let reg = registry();
    let Some(def) = reg.get(service_id) else {
        return unknown_service(service_id);
    };
    let version: ServiceVersion = match version.parse() {
        Ok(v) => v,
        Err(e) => return service_error_response(&e),
    };
    let _ = state.service_manager.lock().await.stop(def.id()).await;
    match service_install::uninstall(
        def.id(),
        def.datadir_pinned_to_major(),
        &version,
        &state.dirs,
        purge,
    ) {
        Ok(retained) => {
            if let Some(path) = retained {
                tracing::info!(service = def.id(), datadir = %path.display(),
                    "uninstalled service; datadir retained (use --purge to delete)");
            }
            Response::Ok
        }
        Err(e) => service_error_response(&e),
    }
}

/// `add-service` - add a new instance. For a versioned type this installs the
/// version; for a per-site type it links a Laravel site. The instance is
/// persisted *before* the start attempt, so a failed start still renders a row.
/// A type with a [`proxy_path`](yerd_services::ServiceDefinition::proxy_path)
/// (Reverb) also gets its reverse-proxy rule set up on the linked site.
#[allow(clippy::too_many_lines)]
pub async fn add_service(
    type_id: &str,
    site: Option<&str>,
    port: Option<u16>,
    version: Option<&str>,
    autostart: Option<bool>,
    state: &DaemonState,
    dl: &dyn Downloader,
) -> Response {
    let reg = registry();
    let Some(def) = reg.get(type_id) else {
        return unknown_service_type(type_id);
    };
    let autostart = autostart.unwrap_or(def.default_autostart());

    let (site_name, program_override, cwd) = if def.requires_site() {
        let Some(site_name) = site else {
            return err(
                ErrorCode::SiteNotFound,
                "this service requires a linked site",
            );
        };
        let (doc_root, php) = {
            let router = state.router.read().await;
            match router.get(site_name) {
                Some(s) => (s.document_root().to_path_buf(), s.php()),
                None => {
                    return err(
                        ErrorCode::SiteNotFound,
                        &format!("unknown site {site_name:?}"),
                    )
                }
            }
        };
        if !crate::laravel_detect::is_laravel(&doc_root) {
            return err(
                ErrorCode::SiteNotLaravel,
                &format!("site {site_name:?} is not a Laravel app (no artisan file)"),
            );
        }
        let php_cli = crate::php_install::cli_binary_path(&state.dirs, php);
        (Some(site_name.to_owned()), Some(php_cli), Some(doc_root))
    } else {
        (None, None, None)
    };

    let wire = wire_id(def.id(), site_name.as_deref());

    {
        let cfg = state.config.lock().await;
        let exists = cfg.services.instances.contains_key(&wire)
            || (matches!(def.multiplicity(), Multiplicity::Single)
                && !installed_versions(def.id(), &state.dirs).is_empty());
        if exists {
            return err(
                ErrorCode::InstanceAlreadyExists,
                &format!("{wire} already exists"),
            );
        }
    }

    let reserved = {
        let cfg = state.config.lock().await;
        reserved_ports(&cfg)
    };
    let chosen_port = match port {
        Some(p) => {
            if reserved.contains(&p) {
                return err(
                    ErrorCode::PortReserved,
                    &format!("port {p} is already reserved"),
                );
            }
            p
        }
        None => match pick_free_port(def.default_port(), &reserved) {
            Some(p) => p,
            None => return err(ErrorCode::Internal, "no free port available"),
        },
    };

    let version_obj = if def.requires_version() {
        let vstr = version.unwrap_or_default();
        let v: ServiceVersion = match vstr.parse() {
            Ok(v) => v,
            Err(e) => return service_error_response(&e),
        };
        if let Err(e) =
            service_install::install(def.id(), def.server_binary(), &v, &state.dirs, dl).await
        {
            return service_error_response(&e);
        }
        Some(v)
    } else {
        None
    };

    if let Err(resp) = persist_instance(state, &wire, |inst| {
        inst.version = version_obj.as_ref().map(ToString::to_string);
        inst.port = Some(chosen_port);
        inst.site.clone_from(&site_name);
        inst.enabled = autostart;
    })
    .await
    {
        return resp;
    }

    let outcome = ensure_and_persist(
        state,
        &def,
        &wire,
        version_obj,
        chosen_port,
        program_override,
        cwd,
    )
    .await;

    if let (Some(prefix), Some(site)) = (def.proxy_path(), site_name.as_deref()) {
        set_service_proxy(state, site, prefix, chosen_port).await;
    }

    match outcome {
        Ok(()) => Response::ServiceInstanceId { id: wire },
        Err(resp) => resp,
    }
}

/// `remove-service <wire-id> [--purge]` - remove a per-site (or version-less)
/// instance: stop it and drop its config entry.
pub async fn remove_service(service_id: &str, _purge: bool, state: &DaemonState) -> Response {
    {
        let mut mgr = state.service_manager.lock().await;
        let _ = mgr.stop(service_id).await;
    }
    let resp = {
        let mut cfg = state.config.lock().await;
        cfg.services.instances.remove(service_id);
        save_cfg(&cfg, state)
    };
    // Tear down the auto-managed proxy rule (best-effort, config lock released).
    let (type_id, site) = parse_wire_id(service_id);
    if let (Some(def), Some(site)) = (registry().get(&type_id), site) {
        if let Some(prefix) = def.proxy_path() {
            clear_service_proxy(state, &site, prefix).await;
        }
    }
    resp
}

/// `set-autostart <wire-id> <on|off>`.
pub async fn set_service_autostart(
    service_id: &str,
    enabled: bool,
    state: &DaemonState,
) -> Response {
    persist_instance(state, service_id, |inst| inst.enabled = enabled)
        .await
        .unwrap_or_else(|resp| resp)
}

/// `set-site <wire-id> <new-site>` - re-link a per-site instance. Stops the
/// instance, moves its config entry under the new wire id, and returns the new
/// id so the client can re-target.
pub async fn set_service_site(service_id: &str, new_site: &str, state: &DaemonState) -> Response {
    let (type_id, old_site) = parse_wire_id(service_id);
    let reg = registry();
    let Some(def) = reg.get(&type_id) else {
        return unknown_service_type(&type_id);
    };
    if old_site.is_none() || !def.requires_site() {
        return service_type_mismatch(service_id, "is not a per-site service");
    }
    // Validate the new site: exists + Laravel.
    let doc_root = {
        let router = state.router.read().await;
        match router.get(new_site) {
            Some(s) => s.document_root().to_path_buf(),
            None => {
                return err(
                    ErrorCode::SiteNotFound,
                    &format!("unknown site {new_site:?}"),
                )
            }
        }
    };
    if !crate::laravel_detect::is_laravel(&doc_root) {
        return err(
            ErrorCode::SiteNotLaravel,
            &format!("site {new_site:?} is not a Laravel app"),
        );
    }
    let new_wire = wire_id(&type_id, Some(new_site));
    {
        let cfg = state.config.lock().await;
        if cfg.services.instances.contains_key(&new_wire) {
            return err(
                ErrorCode::InstanceAlreadyExists,
                &format!("{new_wire} already exists"),
            );
        }
    }
    {
        let mut mgr = state.service_manager.lock().await;
        let _ = mgr.stop(service_id).await;
    }
    {
        let mut cfg = state.config.lock().await;
        if let Some(mut inst) = cfg.services.instances.remove(service_id) {
            inst.site = Some(new_site.to_owned());
            cfg.services.instances.insert(new_wire.clone(), inst);
        }
        if let Response::Error { .. } = save_cfg(&cfg, state) {
            return err(ErrorCode::Internal, "persist config after re-link");
        }
    }
    // Move the auto-managed proxy rule from the old site to the new one.
    if let Some(prefix) = def.proxy_path() {
        let port = {
            let cfg = state.config.lock().await;
            cfg.services
                .instances
                .get(&new_wire)
                .and_then(|i| i.port)
                .unwrap_or(def.default_port())
        };
        if let Some(old) = old_site.as_deref() {
            clear_service_proxy(state, old, prefix).await;
        }
        set_service_proxy(state, new_site, prefix, port).await;
    }
    Response::ServiceInstanceId { id: new_wire }
}

/// `start service <wire-id>` - ensure it's running. Does not change autostart.
pub async fn start_service(service_id: &str, state: &DaemonState) -> Response {
    let (type_id, site) = parse_wire_id(service_id);
    let reg = registry();
    let Some(def) = reg.get(&type_id) else {
        return unknown_service(service_id);
    };

    if def.requires_site() {
        return start_per_site(&def, service_id, site.as_deref(), state).await;
    }

    let (configured_version, port) = {
        let cfg = state.config.lock().await;
        let inst = cfg.services.instances.get(service_id);
        (
            inst.and_then(|i| i.version.clone()),
            inst.and_then(|i| i.port).unwrap_or(def.default_port()),
        )
    };
    let version = match resolve_version(&def, configured_version.as_deref(), &state.dirs) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    match ensure_and_persist(state, &def, service_id, Some(version), port, None, None).await {
        Ok(()) => Response::Ok,
        Err(resp) => resp,
    }
}

/// Start a per-site instance (Reverb): resolve its linked site's PHP + docroot.
async fn start_per_site(
    def: &Arc<dyn ServiceDefinition>,
    wire: &str,
    site: Option<&str>,
    state: &DaemonState,
) -> Response {
    let Some(site_name) = site else {
        return err(
            ErrorCode::SiteNotFound,
            "per-site instance has no linked site",
        );
    };
    let (doc_root, php) = {
        let router = state.router.read().await;
        match router.get(site_name) {
            Some(s) => (s.document_root().to_path_buf(), s.php()),
            None => {
                return err(
                    ErrorCode::SiteNotFound,
                    &format!("unknown site {site_name:?}"),
                )
            }
        }
    };
    let port = {
        let cfg = state.config.lock().await;
        cfg.services
            .instances
            .get(wire)
            .and_then(|i| i.port)
            .unwrap_or(def.default_port())
    };
    let php_cli = crate::php_install::cli_binary_path(&state.dirs, php);
    match ensure_and_persist(state, def, wire, None, port, Some(php_cli), Some(doc_root)).await {
        Ok(()) => Response::Ok,
        Err(resp) => resp,
    }
}

/// `stop service <wire-id>` - stop it. Does NOT change its autostart preference.
pub async fn stop_service(service_id: &str, state: &DaemonState) -> Response {
    let (type_id, _) = parse_wire_id(service_id);
    if registry().get(&type_id).is_none() {
        return unknown_service(service_id);
    }
    let mut mgr = state.service_manager.lock().await;
    match mgr.stop(service_id).await {
        Ok(()) => Response::Ok,
        Err(e) => service_error_response(&e),
    }
}

/// `restart service <wire-id>` - stop + ensure with the configured version.
pub async fn restart_service(service_id: &str, state: &DaemonState) -> Response {
    let (type_id, site) = parse_wire_id(service_id);
    let reg = registry();
    let Some(def) = reg.get(&type_id) else {
        return unknown_service(service_id);
    };
    {
        let mut mgr = state.service_manager.lock().await;
        let _ = mgr.stop(service_id).await;
    }
    if def.requires_site() {
        return start_per_site(&def, service_id, site.as_deref(), state).await;
    }
    let (configured_version, port) = {
        let cfg = state.config.lock().await;
        let inst = cfg.services.instances.get(service_id);
        (
            inst.and_then(|i| i.version.clone()),
            inst.and_then(|i| i.port).unwrap_or(def.default_port()),
        )
    };
    let version = match resolve_version(&def, configured_version.as_deref(), &state.dirs) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let outcome = {
        let mut mgr = state.service_manager.lock().await;
        mgr.ensure(
            Arc::clone(&def),
            service_id,
            Some(version),
            port,
            None,
            None,
        )
        .await
    };
    match outcome {
        Ok(_) => Response::Ok,
        Err(e) => service_error_response(&e),
    }
}

/// `set-port <wire-id> <port>` - validate against reserved ports, then persist.
pub async fn set_service_port(service_id: &str, port: u16, state: &DaemonState) -> Response {
    let (type_id, site) = parse_wire_id(service_id);
    let Some(def) = registry().get(&type_id) else {
        return unknown_service(service_id);
    };
    {
        let cfg = state.config.lock().await;
        let own = cfg.services.instances.get(service_id).and_then(|i| i.port);
        let mut reserved = reserved_ports(&cfg);
        if let Some(p) = own {
            reserved.remove(&p);
        }
        if reserved.contains(&port) {
            return err(
                ErrorCode::PortInUse,
                &format!("port {port} is already in use"),
            );
        }
    }
    let resp = persist_instance(state, service_id, |inst| inst.port = Some(port))
        .await
        .unwrap_or_else(|resp| resp);
    // Keep the auto-managed proxy pointing at the new port.
    if matches!(resp, Response::Ok) {
        if let (Some(prefix), Some(site)) = (def.proxy_path(), site.as_deref()) {
            set_service_proxy(state, site, prefix, port).await;
        }
    }
    resp
}

/// `service logs <wire-id>` - the last `lines` lines of the instance log file.
pub fn service_logs(service_id: &str, lines: u32, state: &DaemonState) -> Response {
    if !valid_instance_id(service_id) {
        return unknown_service(service_id);
    }
    let path = svc_version::instance_log_path(&state.dirs, service_id);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Response::Error {
                code: ErrorCode::Internal,
                message: format!("read {service_id} log: {e}"),
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

/// Build the per-service status list: one row per single-instance type
/// (unconditionally, so uninstalled engines still appear) plus one row per
/// configured per-site instance.
pub async fn service_statuses(state: &DaemonState) -> Vec<ServiceStatus> {
    let snapshots = {
        let mut mgr = state.service_manager.lock().await;
        mgr.snapshots()
    };
    let instances = {
        let cfg = state.config.lock().await;
        cfg.services.instances.clone()
    };
    let reg = registry();
    let installed = svc_version::discover_installed(&state.dirs, &reg).unwrap_or_default();
    let versions_of = |id: &str| installed.get(id).map_or(&[][..], Vec::as_slice);
    let mut out = Vec::new();

    for def in reg.single_instance() {
        let inst = instances.get(def.id());
        out.push(build_status(
            def,
            def.id(),
            None,
            inst,
            &snapshots,
            versions_of(def.id()),
        ));
    }
    for (wire, inst) in &instances {
        let (type_id, site) = parse_wire_id(wire);
        let Some(def) = reg.get(&type_id) else {
            continue;
        };
        if !matches!(def.multiplicity(), Multiplicity::PerSite) {
            continue;
        }
        out.push(build_status(
            &def,
            wire,
            site,
            Some(inst),
            &snapshots,
            versions_of(def.id()),
        ));
    }
    out
}

/// Assemble one [`ServiceStatus`] row for `wire`, given the type's already-scanned
/// installed versions (the caller scans once for the whole list).
fn build_status(
    def: &Arc<dyn ServiceDefinition>,
    wire: &str,
    site: Option<String>,
    inst: Option<&ServiceInstance>,
    snapshots: &[yerd_services::ServiceSnapshot],
    versions: &[ServiceVersion],
) -> ServiceStatus {
    let snap = snapshots.iter().find(|s| s.service == wire);
    let (run_state, pid, listen) = match snap {
        Some(s) => (
            map_run_state(s.state),
            s.pid,
            s.listen.as_ref().map(ToString::to_string),
        ),
        None => (ServiceRunState::Stopped, None, None),
    };
    let error = if run_state == ServiceRunState::Failed {
        Some("the service exited unexpectedly; view its logs for details".to_string())
    } else {
        None
    };
    ServiceStatus {
        service: wire.to_string(),
        display_name: def.display_name().to_string(),
        installed_versions: versions.iter().map(ToString::to_string).collect(),
        selected_version: inst.and_then(|i| i.version.clone()),
        state: run_state,
        pid,
        listen,
        port: inst.and_then(|i| i.port).unwrap_or(def.default_port()),
        enabled: inst.is_some_and(|i| i.enabled),
        supports_databases: def.as_database().is_some(),
        type_id: def.id().to_string(),
        site,
        error,
    }
}

/// Auto-start every instance whose `enabled` flag is set. Runs as a background
/// task so a slow/failing cold-boot never blocks the proxy/DNS listeners.
///
/// Boot autostart now **honours `enabled`**: an engine whose last user action
/// was `Stop` (persisted `enabled=false`) stays stopped; single-instance engines
/// default `enabled=true`, per-site app servers `false`.
pub async fn auto_start_installed(state: Arc<DaemonState>) {
    let enabled: Vec<String> = {
        let cfg = state.config.lock().await;
        cfg.services
            .instances
            .iter()
            .filter(|(_, inst)| inst.enabled)
            .map(|(wire, _)| wire.clone())
            .collect()
    };
    let mut shutdown = state.shutdown_tx.subscribe();
    run_auto_start(enabled, &mut shutdown, |wire| {
        let state = state.clone();
        async move { start_one(&wire, &state).await }
    })
    .await;
}

/// Start each enabled instance in order, stopping the moment `shutdown` trips.
async fn run_auto_start<F, Fut>(
    instances: Vec<String>,
    shutdown: &mut watch::Receiver<bool>,
    mut start_one: F,
) where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), ServiceError>>,
{
    if *shutdown.borrow() {
        return;
    }
    for wire in instances {
        tokio::select! {
            biased;
            _ = shutdown.changed() => return,
            res = start_one(wire.clone()) => match res {
                Ok(()) => tracing::info!(service = %wire, "auto-started service"),
                Err(e) => tracing::warn!(service = %wire, error = %e, "service auto-start failed"),
            },
        }
    }
}

/// Ensure one enabled instance is running (used by auto-start).
async fn start_one(wire: &str, state: &DaemonState) -> Result<(), ServiceError> {
    let (type_id, site) = parse_wire_id(wire);
    let reg = registry();
    let Some(def) = reg.get(&type_id) else {
        return Ok(());
    };

    if def.requires_site() {
        let Some(site_name) = site else {
            return Ok(());
        };
        let resolved = {
            let router = state.router.read().await;
            router
                .get(&site_name)
                .map(|s| (s.document_root().to_path_buf(), s.php()))
        };
        let Some((doc_root, php)) = resolved else {
            return Ok(());
        };
        let port = {
            let cfg = state.config.lock().await;
            cfg.services
                .instances
                .get(wire)
                .and_then(|i| i.port)
                .unwrap_or(def.default_port())
        };
        let php_cli = crate::php_install::cli_binary_path(&state.dirs, php);
        let mut mgr = state.service_manager.lock().await;
        return mgr
            .ensure(def, wire, None, port, Some(php_cli), Some(doc_root))
            .await
            .map(|_| ());
    }

    let (configured_version, port) = {
        let cfg = state.config.lock().await;
        let inst = cfg.services.instances.get(wire);
        (
            inst.and_then(|i| i.version.clone()),
            inst.and_then(|i| i.port).unwrap_or(def.default_port()),
        )
    };
    let version =
        match configured_version {
            Some(v) => v.parse::<ServiceVersion>()?,
            None => installed_versions(def.id(), &state.dirs).pop().ok_or(
                ServiceError::Unsupported {
                    service: wire.to_owned(),
                    detail: "no installed version to auto-start".to_owned(),
                },
            )?,
        };
    let mut mgr = state.service_manager.lock().await;
    mgr.ensure(def, wire, Some(version), port, None, None)
        .await
        .map(|_| ())
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Every port already spoken for by config: each service instance's configured
/// or default port, plus mail / dumps / DNS / HTTP(S) and their fallbacks.
fn reserved_ports(cfg: &Config) -> BTreeSet<u16> {
    let reg = registry();
    let mut r = BTreeSet::new();
    for (wire, inst) in &cfg.services.instances {
        let (ty, _) = parse_wire_id(wire);
        let default = reg.get(&ty).map_or(0, |d| d.default_port());
        r.insert(inst.port.unwrap_or(default));
    }
    r.insert(cfg.mail.port);
    r.insert(cfg.dumps.port);
    r.insert(cfg.dns_port);
    r.insert(cfg.ports.http);
    r.insert(cfg.ports.https);
    r.insert(cfg.ports.fallback_http);
    r.insert(cfg.ports.fallback_https);
    r.remove(&0);
    r
}

/// Walk candidate ports from `start`, skipping `reserved`, returning the first
/// that binds on loopback.
fn pick_free_port(start: u16, reserved: &BTreeSet<u16>) -> Option<u16> {
    let binder = ActivePortBinder::new();
    candidate_ports(start, reserved).find(|p| binder.bind(*p).map(drop).is_ok())
}

/// Add or re-target the auto-managed reverse-proxy rule for a per-site service
/// (e.g. Reverb's `/app` -> the instance's loopback port) on `site`. Removes any
/// existing rule for the same prefix first, so a re-target/move takes effect
/// cleanly. Best-effort: a proxy failure is logged, never fails the service op.
/// The caller must NOT hold the config or service-manager lock (this re-enters
/// the mutation path, which locks config and rebuilds the router).
async fn set_service_proxy(state: &DaemonState, site: &str, prefix: &str, port: u16) {
    let _ = crate::ipc_server::handle_mutation(
        Request::RemoveProxyRule {
            site: site.to_owned(),
            prefix: prefix.to_owned(),
        },
        state,
    )
    .await;
    let resp = crate::ipc_server::handle_mutation(
        Request::AddProxyRule {
            site: site.to_owned(),
            prefix: prefix.to_owned(),
            url: format!("http://127.0.0.1:{port}"),
        },
        state,
    )
    .await;
    if let Response::Error { message, .. } = resp {
        tracing::warn!(site, prefix, port, %message, "couldn't set the service's proxy rule");
    }
}

/// Remove the auto-managed proxy rule for a per-site service from `site`.
/// Best-effort; the caller must not hold the config/manager lock.
async fn clear_service_proxy(state: &DaemonState, site: &str, prefix: &str) {
    let _ = crate::ipc_server::handle_mutation(
        Request::RemoveProxyRule {
            site: site.to_owned(),
            prefix: prefix.to_owned(),
        },
        state,
    )
    .await;
}

/// Installed versions of `type_id`, ascending.
fn installed_versions(type_id: &str, dirs: &PlatformDirs) -> Vec<ServiceVersion> {
    svc_version::discover_installed(dirs, &registry())
        .ok()
        .and_then(|mut m| m.remove(type_id))
        .unwrap_or_default()
}

/// Resolve the version to run: the configured one if installed, else the latest
/// installed; error if nothing is installed.
pub(crate) fn resolve_version(
    def: &Arc<dyn ServiceDefinition>,
    configured: Option<&str>,
    dirs: &PlatformDirs,
) -> Result<ServiceVersion, Response> {
    let mut installed = installed_versions(def.id(), dirs);
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
            def.display_name(),
            def.id()
        ),
    })
}

/// Apply a mutation to a service instance's config, validate, and persist.
async fn persist_instance(
    state: &DaemonState,
    wire: &str,
    f: impl FnOnce(&mut ServiceInstance),
) -> Result<Response, Response> {
    let mut cfg = state.config.lock().await;
    let (_, site) = parse_wire_id(wire);
    let inst = cfg
        .services
        .instances
        .entry(wire.to_string())
        .or_insert_with(|| ServiceInstance {
            site: site.clone(),
            ..ServiceInstance::default()
        });
    f(inst);
    match save_cfg(&cfg, state) {
        Response::Ok => Ok(Response::Ok),
        other => Err(other),
    }
}

/// Validate + save the config, mapping failures to a `Response::Error`.
fn save_cfg(cfg: &Config, state: &DaemonState) -> Response {
    if let Err(e) = cfg.validate() {
        return Response::Error {
            code: ErrorCode::Internal,
            message: format!("config validation failed: {e}"),
        };
    }
    if let Err(e) = cfg.save(&state.config_path) {
        return Response::Error {
            code: ErrorCode::Internal,
            message: format!("persist config: {e}"),
        };
    }
    Response::Ok
}

fn map_run_state(s: MgrRunState) -> ServiceRunState {
    match s {
        MgrRunState::Running => ServiceRunState::Running,
        MgrRunState::Failed => ServiceRunState::Failed,
    }
}

fn err(code: ErrorCode, message: &str) -> Response {
    Response::Error {
        code,
        message: message.to_string(),
    }
}

fn unknown_service(id: &str) -> Response {
    err(ErrorCode::NotFound, &format!("unknown service {id:?}"))
}

fn unknown_service_type(id: &str) -> Response {
    err(
        ErrorCode::UnknownServiceType,
        &format!("unknown service type {id:?}"),
    )
}

fn service_type_mismatch(id: &str, why: &str) -> Response {
    err(ErrorCode::InvalidPath, &format!("service {id:?} {why}"))
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
    use yerd_platform::PlatformDirs;

    use crate::test_support::{dirs_in, state_in};

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

    fn def_of(id: &str) -> Arc<dyn ServiceDefinition> {
        registry().get(id).unwrap()
    }

    fn install_fake(dirs: &PlatformDirs, type_id: &str, version: &str) {
        let def = def_of(type_id);
        let ver: ServiceVersion = version.parse().unwrap();
        let bin = svc_version::install_dir(dirs, def.id(), &ver).join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(def.server_binary().unwrap()), b"#!fake").unwrap();
    }

    fn ver(s: &str) -> ServiceVersion {
        s.parse().unwrap()
    }

    fn err_parts(r: Response) -> (ErrorCode, String) {
        match r {
            Response::Error { code, message } => (code, message),
            other => panic!("expected Response::Error, got {other:?}"),
        }
    }

    #[test]
    fn wire_id_round_trips() {
        assert_eq!(wire_id("redis", None), "redis");
        assert_eq!(wire_id("reverb", Some("blog")), "reverb:blog");
        assert_eq!(parse_wire_id("redis"), ("redis".to_owned(), None));
        assert_eq!(
            parse_wire_id("reverb:blog"),
            ("reverb".to_owned(), Some("blog".to_owned()))
        );
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
    fn installed_versions_discovers_laid_down_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        install_fake(&dirs, "mariadb", "11.4");
        assert_eq!(installed_versions("mariadb", &dirs), vec![ver("11.4")]);
        assert!(installed_versions("mysql", &dirs).is_empty());
    }

    #[test]
    fn resolve_version_errors_when_nothing_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        match resolve_version(&def_of("redis"), None, &dirs) {
            Err(Response::Error { code, message }) => {
                assert_eq!(code, ErrorCode::NotFound);
                assert!(message.contains("redis"));
            }
            other => panic!("expected NotFound error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_version_prefers_configured_then_latest() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        install_fake(&dirs, "postgres", "16.2");
        install_fake(&dirs, "postgres", "17.0");
        assert_eq!(
            resolve_version(&def_of("postgres"), Some("16.2"), &dirs).unwrap(),
            ver("16.2")
        );
        assert_eq!(
            resolve_version(&def_of("postgres"), Some("99.0"), &dirs).unwrap(),
            ver("17.0")
        );
    }

    #[test]
    fn reserved_ports_includes_defaults_and_infra() {
        let mut cfg = Config::default();
        cfg.services
            .instances
            .insert("redis".to_string(), ServiceInstance::default());
        let r = reserved_ports(&cfg);
        assert!(r.contains(&6379), "redis default port reserved: {r:?}");
        assert!(r.contains(&cfg.dns_port));
        assert!(r.contains(&cfg.ports.http));
    }

    #[test]
    fn pick_free_port_skips_reserved() {
        let mut reserved = BTreeSet::new();
        reserved.insert(8080);
        let p = pick_free_port(8080, &reserved).unwrap();
        assert_ne!(p, 8080);
        assert!(p >= 8081);
    }

    #[tokio::test]
    async fn service_statuses_lists_single_instance_types_always() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let statuses = service_statuses(&state).await;
        assert_eq!(statuses.len(), registry().single_instance().count());
        for s in &statuses {
            assert_eq!(s.state, ServiceRunState::Stopped);
            assert!(!s.enabled);
        }
        let mysql = statuses.iter().find(|s| s.service == "mysql").unwrap();
        assert!(mysql.supports_databases);
        assert_eq!(mysql.type_id, "mysql");
        let redis = statuses.iter().find(|s| s.service == "redis").unwrap();
        assert!(!redis.supports_databases);
    }

    #[tokio::test]
    async fn service_statuses_includes_configured_per_site_instances() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        {
            let mut cfg = state.config.lock().await;
            cfg.services.instances.insert(
                "reverb:blog".to_string(),
                ServiceInstance {
                    site: Some("blog".to_string()),
                    port: Some(8081),
                    enabled: false,
                    ..ServiceInstance::default()
                },
            );
        }
        let statuses = service_statuses(&state).await;
        let reverb = statuses
            .iter()
            .find(|s| s.service == "reverb:blog")
            .unwrap();
        assert_eq!(reverb.type_id, "reverb");
        assert_eq!(reverb.site.as_deref(), Some("blog"));
        assert_eq!(reverb.port, 8081);
    }

    #[tokio::test]
    async fn available_services_excludes_versionless_types() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let dl = FakeDownloader {
            body: Some(b"{\"schema\":1,\"services\":{}}".to_vec()),
        };
        match available_services(&state, &dl).await {
            Response::AvailableServices { services } => {
                assert!(services.iter().all(|s| s.service != "reverb"));
                assert_eq!(
                    services.len(),
                    registry().iter().filter(|d| d.requires_version()).count()
                );
            }
            other => panic!("expected AvailableServices, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stop_service_does_not_change_autostart() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        {
            let mut cfg = state.config.lock().await;
            cfg.services.instances.insert(
                "redis".to_string(),
                ServiceInstance {
                    enabled: true,
                    ..ServiceInstance::default()
                },
            );
        }
        assert!(matches!(stop_service("redis", &state).await, Response::Ok));
        let cfg = state.config.lock().await;
        assert!(
            cfg.services.instances.get("redis").unwrap().enabled,
            "stop must not clear the autostart flag"
        );
    }

    #[tokio::test]
    async fn set_service_autostart_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        assert!(matches!(
            set_service_autostart("redis", false, &state).await,
            Response::Ok
        ));
        let cfg = state.config.lock().await;
        assert!(!cfg.services.instances.get("redis").unwrap().enabled);
    }

    #[tokio::test]
    async fn unknown_service_type_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let dl = FakeDownloader { body: None };
        let (code, _) = err_parts(add_service("nope", None, None, None, Some(true), &state, &dl).await);
        assert_eq!(code, ErrorCode::UnknownServiceType);
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
    async fn service_logs_tails_last_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let path = svc_version::log_path(&state.dirs, "mysql");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"l1\nl2\nl3\nl4\nl5\n").unwrap();
        match service_logs("mysql", 2, &state) {
            Response::ServiceLogs { lines } => assert_eq!(lines, vec!["l4", "l5"]),
            other => panic!("expected ServiceLogs, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn auto_start_installed_is_noop_with_nothing_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let state = std::sync::Arc::new(state_in(tmp.path()));
        auto_start_installed(state).await;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod auto_start_tests {
    use std::sync::Mutex;

    use super::{run_auto_start, watch, Arc, ServiceError};

    #[tokio::test]
    async fn skips_everything_when_shutdown_already_requested() {
        let (_tx, mut rx) = watch::channel(true);
        let started: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let rec = started.clone();
        run_auto_start(
            vec!["redis".to_string(), "mysql".to_string()],
            &mut rx,
            move |w| {
                let rec = rec.clone();
                async move {
                    rec.lock().unwrap().push(w);
                    Ok::<(), ServiceError>(())
                }
            },
        )
        .await;
        assert!(started.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn stops_after_shutdown_trips_mid_loop() {
        let (tx, mut rx) = watch::channel(false);
        let tx = Arc::new(tx);
        let started: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let rec = started.clone();
        run_auto_start(
            vec!["redis".to_string(), "mysql".to_string()],
            &mut rx,
            move |w| {
                let rec = rec.clone();
                let tx = tx.clone();
                async move {
                    rec.lock().unwrap().push(w);
                    let _ = tx.send(true);
                    Ok::<(), ServiceError>(())
                }
            },
        )
        .await;
        let started = started.lock().unwrap();
        assert_eq!(started.len(), 1, "only the first instance should start");
        assert_eq!(started[0], "redis");
    }
}
