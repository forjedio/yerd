//! Tauri commands: one per `yerd-ipc` Request, plus a few host-only helpers.
//!
//! Every daemon command maps `command → Request`, calls [`crate::ipc::exchange`],
//! and converts a `Response::Error` into a [`GuiError`] so the frontend only
//! ever sees a success variant or a typed failure. There is no business logic
//! here - that lives in the daemon and its crates (the thin-client rule).

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::Manager;
use yerd_core::PhpVersion;
use yerd_ipc::{ErrorCode, Request, Response};

use crate::error::GuiError;
use crate::ipc::{exchange, exchange_timeout};

/// Bound for the liveness/probe commands (`status`/`ping`/`daemon_info`): a
/// healthy in-memory reply returns in ms (the daemon serves connections
/// concurrently, so an in-flight install doesn't block it), so 5 s only ever
/// trips for a wedged/crash-looping daemon - letting the poller advance instead
/// of hanging. Heavy/mutating commands deliberately stay unbounded.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Convert a daemon `Response::Error` into a `GuiError`; pass success through.
fn finish(resp: Response) -> Result<Response, GuiError> {
    if let Response::Error { code, message } = &resp {
        return Err(GuiError::daemon(code_str(code), message.clone()));
    }
    Ok(resp)
}

/// Render an `ErrorCode` as its snake_case wire string (via serde so a new
/// variant doesn't need a match arm here).
fn code_str(code: &ErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "internal".to_owned())
}

// ── liveness ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ping() -> Result<Response, GuiError> {
    finish(exchange_timeout(&Request::Ping, PROBE_TIMEOUT).await?)
}

// ── sites ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_sites() -> Result<Response, GuiError> {
    finish(exchange(&Request::ListSites).await?)
}

#[tauri::command]
pub async fn park(path: String) -> Result<Response, GuiError> {
    finish(
        exchange(&Request::Park {
            path: PathBuf::from(path),
        })
        .await?,
    )
}

#[tauri::command]
pub async fn link(name: String, path: String) -> Result<Response, GuiError> {
    finish(
        exchange(&Request::Link {
            name,
            path: PathBuf::from(path),
        })
        .await?,
    )
}

#[tauri::command]
pub async fn unlink(name: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::Unlink { name }).await?)
}

#[tauri::command]
pub async fn list_parked() -> Result<Response, GuiError> {
    finish(exchange(&Request::ListParked).await?)
}

#[tauri::command]
pub async fn unpark(path: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::Unpark { path }).await?)
}

#[tauri::command]
pub async fn set_php(name: String, version: PhpVersion) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetPhp { name, version }).await?)
}

#[tauri::command]
pub async fn set_secure(name: String, secure: bool) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetSecure { name, secure }).await?)
}

#[tauri::command]
pub async fn set_web_root(name: String, path: Option<String>) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetWebRoot { name, path }).await?)
}

// ── domains ────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn add_domain(name: String, domain: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::AddDomain { name, domain }).await?)
}

#[tauri::command]
pub async fn remove_domain(name: String, domain: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::RemoveDomain { name, domain }).await?)
}

#[tauri::command]
pub async fn set_primary_domain(name: String, domain: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetPrimaryDomain { name, domain }).await?)
}

#[tauri::command]
pub async fn reset_domains(name: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::ResetDomains { name }).await?)
}

// ── proxies ────────────────────────────────────────────────────────────────

/// List every whole-host reverse proxy and per-site path-prefix rule.
#[tauri::command]
pub async fn list_proxies() -> Result<Response, GuiError> {
    finish(exchange(&Request::ListProxies).await?)
}

/// Register a whole-host reverse proxy (`{name}.{tld}` → `url`).
#[tauri::command]
pub async fn add_proxy(name: String, url: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::AddProxy { name, url }).await?)
}

/// Remove the whole-host reverse proxy named `name`.
#[tauri::command]
pub async fn remove_proxy(name: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::RemoveProxy { name }).await?)
}

/// Add a path-prefix rule to `site` (`site/prefix` → `url`), leaving other paths
/// served by PHP.
#[tauri::command]
pub async fn add_proxy_rule(
    site: String,
    prefix: String,
    url: String,
) -> Result<Response, GuiError> {
    finish(exchange(&Request::AddProxyRule { site, prefix, url }).await?)
}

/// Remove the path-prefix rule `prefix` from `site`.
#[tauri::command]
pub async fn remove_proxy_rule(site: String, prefix: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::RemoveProxyRule { site, prefix }).await?)
}

// ── site groups ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_groups() -> Result<Response, GuiError> {
    finish(exchange(&Request::ListGroups).await?)
}

#[tauri::command]
pub async fn create_group(name: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::CreateGroup { name }).await?)
}

#[tauri::command]
pub async fn delete_group(name: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::DeleteGroup { name }).await?)
}

#[tauri::command]
pub async fn set_group_order(order: Vec<String>) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetGroupOrder { order }).await?)
}

#[tauri::command]
pub async fn set_site_group(site: String, group: Option<String>) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetSiteGroup { site, group }).await?)
}

#[tauri::command]
pub async fn rename_group(from: String, to: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::RenameGroup { from, to }).await?)
}

// ── php versions ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_php() -> Result<Response, GuiError> {
    finish(exchange(&Request::ListPhp).await?)
}

#[tauri::command]
pub async fn check_php_updates() -> Result<Response, GuiError> {
    finish(exchange(&Request::CheckPhpUpdates).await?)
}

#[tauri::command]
pub async fn available_php() -> Result<Response, GuiError> {
    finish(exchange(&Request::AvailablePhp).await?)
}

#[tauri::command]
pub async fn install_php(version: PhpVersion) -> Result<Response, GuiError> {
    finish(exchange(&Request::InstallPhp { version }).await?)
}

/// Start a streamed PHP install; replies `JobStarted` for the client to poll via
/// `job_status`. The non-blocking sibling of `install_php`, used by the GUI so a
/// multi-minute download streams progress instead of spinning a single request.
#[tauri::command]
pub async fn install_php_streamed(version: PhpVersion) -> Result<Response, GuiError> {
    finish(exchange(&Request::InstallPhpStreamed { version }).await?)
}

#[tauri::command]
pub async fn set_default_php(version: PhpVersion) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetDefaultPhp { version }).await?)
}

#[tauri::command]
pub async fn update_php(version: Option<PhpVersion>) -> Result<Response, GuiError> {
    finish(exchange(&Request::UpdatePhp { version }).await?)
}

// ── self-update ────────────────────────────────────────────────────────────

/// Parse a channel string (`"stable"` / `"edge"`) from the frontend.
fn parse_channel(s: &str) -> Result<yerd_ipc::Channel, GuiError> {
    match s {
        "stable" => Ok(yerd_ipc::Channel::Stable),
        "edge" => Ok(yerd_ipc::Channel::Edge),
        other => Err(GuiError::internal(format!(
            "unknown update channel: {other}"
        ))),
    }
}

/// Check for a Yerd self-update. `channel` (`"stable"`/`"edge"`) overrides the
/// saved preference for this check only; omit to use the saved default.
#[tauri::command]
pub async fn check_updates(channel: Option<String>) -> Result<Response, GuiError> {
    let channel = channel.as_deref().map(parse_channel).transpose()?;
    finish(exchange(&Request::CheckUpdate { channel }).await?)
}

/// Return the last persisted update-check result (no network) to pre-fill the UI.
#[tauri::command]
pub async fn cached_update_status() -> Result<Response, GuiError> {
    finish(exchange(&Request::CachedUpdateStatus).await?)
}

/// Persist the self-update channel preference.
#[tauri::command]
pub async fn set_update_channel(channel: String) -> Result<Response, GuiError> {
    let channel = parse_channel(&channel)?;
    finish(exchange(&Request::SetUpdateChannel { channel }).await?)
}

/// Download + verify the latest update (via the daemon), then launch the
/// detached applier and quit so it can swap this running bundle. The applier
/// relaunches the GUI when it finishes.
///
/// On macOS this needs `/Applications/Yerd.app` to be user-writable (the common
/// admin case); elevated self-update is a follow-up. On Linux the applier uses
/// `pkexec dpkg -i` (`.deb`) or `pkexec pacman -U` (`.pkg.tar.zst`), which prompt
/// via the desktop polkit agent. The `kind_str` mapping below must stay in sync
/// with the `YERD_APPLY_KIND` parser in `bin/yerd/src/apply.rs`.
#[tauri::command]
pub async fn apply_update(app: tauri::AppHandle, channel: Option<String>) -> Result<(), GuiError> {
    let channel = channel.as_deref().map(parse_channel).transpose()?;
    let (path, kind) = match finish(exchange(&Request::StageUpdate { channel }).await?)? {
        Response::Staged { path, kind, .. } => (path, kind),
        _ => return Err(GuiError::internal("unexpected response staging the update")),
    };
    let yerd = crate::daemon::resolve_binary("yerd")
        .ok_or_else(|| GuiError::internal("could not locate the bundled yerd binary"))?;
    let kind_str = match kind {
        yerd_ipc::StagedArtifact::AppTarGz => "app_tar_gz",
        yerd_ipc::StagedArtifact::Deb => "deb",
        yerd_ipc::StagedArtifact::Pacman => "pacman",
        yerd_ipc::StagedArtifact::Rpm => "rpm",
        _ => {
            return Err(GuiError::internal(
                "unknown staged artifact kind from the daemon",
            ))
        }
    };
    spawn_applier(&yerd, &path, kind_str)?;
    app.exit(0);
    Ok(())
}

/// Launch the hidden applier mode of `yerd` detached, via env vars (the contract
/// mirrors `bin/yerd/src/apply.rs`; env names are string literals in both crates
/// since the GUI cannot depend on the `yerd` binary crate).
///
/// macOS: when the daemon is managed via `SMAppService`, the relaunched GUI is
/// the single owner of the launchd re-registration, so `YERD_APPLY_GUI_OWNS_DAEMON`
/// tells the applier not to restart the daemon itself - a second `kickstart -k`
/// would race the GUI's unregister/register (the phantom/EINVAL restart).
#[cfg(unix)]
fn spawn_applier(yerd: &std::path::Path, path: &str, kind: &str) -> Result<(), GuiError> {
    use std::os::unix::process::CommandExt as _;
    let mut cmd = std::process::Command::new(yerd);
    cmd.env("YERD_APPLY_UPDATE", "1")
        .env("YERD_APPLY_PATH", path)
        .env("YERD_APPLY_KIND", kind)
        .env("YERD_APPLY_RELAUNCH_GUI", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0);
    #[cfg(target_os = "macos")]
    if crate::autostart::use_smappservice() {
        cmd.env("YERD_APPLY_GUI_OWNS_DAEMON", "1");
    }
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| GuiError::internal(format!("could not launch the updater: {e}")))
}

#[cfg(not(unix))]
fn spawn_applier(_yerd: &std::path::Path, _path: &str, _kind: &str) -> Result<(), GuiError> {
    Err(GuiError::internal(
        "self-update is not supported on this platform",
    ))
}

#[tauri::command]
pub async fn set_php_settings(
    settings: std::collections::BTreeMap<String, String>,
) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetPhpSettings { settings }).await?)
}

#[tauri::command]
pub async fn set_php_version_settings(
    version: PhpVersion,
    settings: std::collections::BTreeMap<String, String>,
) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetPhpVersionSettings { version, settings }).await?)
}

#[tauri::command]
pub async fn list_php_extensions() -> Result<Response, GuiError> {
    finish(exchange(&Request::ListPhpExtensions).await?)
}

#[tauri::command]
pub async fn add_php_extension(
    version: PhpVersion,
    path: String,
    name: Option<String>,
    zend: bool,
) -> Result<Response, GuiError> {
    finish(
        exchange(&Request::AddPhpExtension {
            version,
            path,
            name,
            zend,
        })
        .await?,
    )
}

#[tauri::command]
pub async fn remove_php_extension(version: PhpVersion, name: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::RemovePhpExtension { version, name }).await?)
}

#[tauri::command]
pub async fn restart_php(version: PhpVersion) -> Result<Response, GuiError> {
    finish(exchange(&Request::RestartPhp { version }).await?)
}

#[tauri::command]
pub async fn restart_all_php() -> Result<Response, GuiError> {
    finish(exchange(&Request::RestartAllPhp).await?)
}

#[tauri::command]
pub async fn uninstall_php(version: PhpVersion) -> Result<Response, GuiError> {
    finish(exchange(&Request::UninstallPhp { version }).await?)
}

#[tauri::command]
pub async fn restart_daemon() -> Result<Response, GuiError> {
    finish(exchange(&Request::RestartDaemon).await?)
}

// ── services (databases / caches) ────────────────────────────────────────────

#[tauri::command]
pub async fn list_services() -> Result<Response, GuiError> {
    finish(exchange(&Request::ListServices).await?)
}

#[tauri::command]
pub async fn available_services() -> Result<Response, GuiError> {
    finish(exchange(&Request::AvailableServices).await?)
}

#[tauri::command]
pub async fn install_service(service: String, version: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::InstallService { service, version }).await?)
}

#[tauri::command]
pub async fn available_wordpress_versions() -> Result<Response, GuiError> {
    finish(exchange(&Request::AvailableWordpressVersions).await?)
}

#[tauri::command]
pub async fn mint_wordpress_login_token(site: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::MintWordpressLoginToken { site }).await?)
}

#[tauri::command]
pub async fn set_wordpress_auto_login(
    name: String,
    enabled: bool,
    user: Option<String>,
) -> Result<Response, GuiError> {
    finish(
        exchange(&Request::SetWordpressAutoLogin {
            name,
            enabled,
            user,
        })
        .await?,
    )
}

#[tauri::command]
pub async fn set_front_controller(name: String, enabled: bool) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetFrontController { name, enabled }).await?)
}

#[tauri::command]
pub async fn wordpress_admin_users(site: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::WordpressAdminUsers { site }).await?)
}

#[tauri::command]
pub async fn change_service_version(
    service: String,
    version: String,
) -> Result<Response, GuiError> {
    finish(exchange(&Request::ChangeServiceVersion { service, version }).await?)
}

#[tauri::command]
pub async fn uninstall_service(
    service: String,
    version: String,
    purge: bool,
) -> Result<Response, GuiError> {
    finish(
        exchange(&Request::UninstallService {
            service,
            version,
            purge,
        })
        .await?,
    )
}

#[tauri::command]
pub async fn start_service(service: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::StartService { service }).await?)
}

#[tauri::command]
pub async fn stop_service(service: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::StopService { service }).await?)
}

#[tauri::command]
pub async fn restart_service(service: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::RestartService { service }).await?)
}

#[tauri::command]
pub async fn set_service_port(service: String, port: u16) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetServicePort { service, port }).await?)
}

#[tauri::command]
pub async fn service_logs(service: String, lines: u32) -> Result<Response, GuiError> {
    finish(exchange(&Request::ServiceLogs { service, lines }).await?)
}

#[tauri::command]
pub async fn addable_service_types() -> Result<Response, GuiError> {
    finish(exchange(&Request::AddableServiceTypes).await?)
}

#[tauri::command]
pub async fn add_service(
    type_id: String,
    site: Option<String>,
    port: Option<u16>,
    version: Option<String>,
    autostart: bool,
) -> Result<Response, GuiError> {
    finish(
        exchange(&Request::AddService {
            type_id,
            site,
            port,
            version,
            autostart: Some(autostart),
        })
        .await?,
    )
}

#[tauri::command]
pub async fn remove_service(service: String, purge: bool) -> Result<Response, GuiError> {
    finish(exchange(&Request::RemoveService { service, purge }).await?)
}

#[tauri::command]
pub async fn set_service_autostart(service: String, enabled: bool) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetServiceAutostart { service, enabled }).await?)
}

#[tauri::command]
pub async fn set_service_site(service: String, site: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetServiceSite { service, site }).await?)
}

#[tauri::command]
pub async fn create_database(service: String, name: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::CreateDatabase { service, name }).await?)
}

#[tauri::command]
pub async fn list_databases(service: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::ListDatabases { service }).await?)
}

#[tauri::command]
pub async fn drop_database(service: String, name: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::DropDatabase { service, name }).await?)
}

#[tauri::command]
pub async fn backup_database(
    service: String,
    name: String,
    path: String,
) -> Result<Response, GuiError> {
    finish(
        exchange(&Request::BackupDatabase {
            service,
            name,
            path: PathBuf::from(path),
        })
        .await?,
    )
}

#[tauri::command]
pub async fn restore_database(
    service: String,
    name: String,
    path: String,
) -> Result<Response, GuiError> {
    finish(
        exchange(&Request::RestoreDatabase {
            service,
            name,
            path: PathBuf::from(path),
        })
        .await?,
    )
}

// ── mail capture ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_mails() -> Result<Response, GuiError> {
    finish(exchange(&Request::ListMails).await?)
}

#[tauri::command]
pub async fn get_mail(id: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::GetMail { id }).await?)
}

#[tauri::command]
pub async fn clear_mails() -> Result<Response, GuiError> {
    finish(exchange(&Request::ClearMails).await?)
}

#[tauri::command]
pub async fn delete_mails(ids: Vec<String>) -> Result<Response, GuiError> {
    finish(exchange(&Request::DeleteMails { ids }).await?)
}

#[tauri::command]
pub async fn mark_mails_read(ids: Vec<String>) -> Result<Response, GuiError> {
    finish(exchange(&Request::MarkMailsRead { ids }).await?)
}

#[tauri::command]
pub async fn set_mail_port(port: u16) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetMailPort { port }).await?)
}

#[tauri::command]
pub async fn set_fallback_ports(http: u16, https: u16) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetFallbackPorts { http, https }).await?)
}

#[tauri::command]
pub async fn set_dns_port(port: u16) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetDnsPort { port }).await?)
}

#[tauri::command]
pub async fn set_mail_enabled(enabled: bool) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetMailEnabled { enabled }).await?)
}

#[tauri::command]
pub async fn set_symlink_protection(enabled: bool) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetSymlinkProtection { enabled }).await?)
}

// ── status / doctor / info ─────────────────────────────────────────────────

#[tauri::command]
pub async fn status() -> Result<Response, GuiError> {
    finish(exchange_timeout(&Request::Status, PROBE_TIMEOUT).await?)
}

#[tauri::command]
pub async fn diagnose() -> Result<Response, GuiError> {
    finish(exchange(&Request::Diagnose).await?)
}

#[tauri::command]
pub async fn doctor_fix() -> Result<Response, GuiError> {
    finish(exchange(&Request::DoctorFix).await?)
}

#[tauri::command]
pub async fn daemon_info() -> Result<Response, GuiError> {
    finish(exchange_timeout(&Request::DaemonInfo, PROBE_TIMEOUT).await?)
}

// ── host-only helpers (no daemon IPC) ──────────────────────────────────────

/// The negotiated IPC protocol version, for the About view.
#[tauri::command]
pub fn protocol_version() -> u32 {
    yerd_ipc::PROTOCOL_VERSION
}

/// The host OS string (`"linux"`, `"macos"`, `"windows"`), to gate platform UI.
#[tauri::command]
pub fn host_platform() -> &'static str {
    std::env::consts::OS
}

/// Run `yerd elevate <target>` under OS elevation. See the plan's elevation
/// section: the GUI never elevates itself; it elevates the audited CLI and
/// threads the real uid through (`pkexec` clears `SUDO_UID`).
#[tauri::command]
pub async fn elevate(target: String) -> Result<(), GuiError> {
    crate::elevate::run("elevate", &target).await
}

/// Run `yerd elevate` with no subcommand - applies every step (trust, resolver,
/// ports) in one OS-elevated invocation.
#[tauri::command]
pub async fn elevate_all() -> Result<(), GuiError> {
    crate::elevate::run("elevate", "").await
}

/// Apply resolver + ports in a **single** OS-elevated prompt. macOS "Fix all"
/// uses this (trust is handled separately in-process) so the user gets one
/// password prompt for the two root steps instead of one each.
#[tauri::command]
pub async fn elevate_resolver_ports() -> Result<(), GuiError> {
    crate::elevate::run_many("elevate", &["resolver", "ports"]).await
}

/// Revert what `elevate` configured: runs `yerd unelevate <target>` under the
/// same OS elevation. On macOS, `unelevate resolver` restores the pre-Yerd
/// resolver from its backup (else removes Yerd's file).
#[tauri::command]
pub async fn unelevate(target: String) -> Result<(), GuiError> {
    crate::elevate::run("unelevate", &target).await
}

/// Trust the local CA for the current user, in-process (macOS only). Unlike
/// `elevate("trust")` this needs no root and prompts as "Yerd"; see `mac_trust`.
#[tauri::command]
pub async fn trust_ca() -> Result<(), GuiError> {
    #[cfg(target_os = "macos")]
    {
        crate::mac_trust::trust_ca().await
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(GuiError::internal(
            "in-app CA trust is only supported on macOS",
        ))
    }
}

/// Remove the current user's trust of the local CA (macOS only). Returns `true`
/// if a system-wide trust set via the terminal still remains (the GUI can't
/// remove that without root).
#[tauri::command]
pub async fn untrust_ca() -> Result<bool, GuiError> {
    #[cfg(target_os = "macos")]
    {
        crate::mac_trust::untrust_ca().await
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(GuiError::internal(
            "in-app CA trust is only supported on macOS",
        ))
    }
}

// ── dumps (Laravel telemetry) ────────────────────────────────────────────────

#[tauri::command]
pub async fn list_dumps(since: u64) -> Result<Response, GuiError> {
    finish(exchange(&Request::ListDumps { since_id: since }).await?)
}

#[tauri::command]
pub async fn clear_dumps() -> Result<Response, GuiError> {
    finish(exchange(&Request::ClearDumps).await?)
}

#[tauri::command]
pub async fn delete_dump(id: u64) -> Result<Response, GuiError> {
    finish(exchange(&Request::DeleteDump { id }).await?)
}

#[tauri::command]
pub async fn set_dumps_enabled(enabled: bool) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetDumpsEnabled { enabled }).await?)
}

#[tauri::command]
pub async fn set_dumps_persist(persist: bool) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetDumpsPersist { persist }).await?)
}

#[tauri::command]
pub async fn set_dumps_port(port: u16) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetDumpsPort { port }).await?)
}

#[tauri::command]
pub async fn set_dump_feature(feature: String, enabled: bool) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetDumpFeature { feature, enabled }).await?)
}

#[tauri::command]
pub async fn dumps_status() -> Result<Response, GuiError> {
    finish(exchange(&Request::DumpsStatus).await?)
}

// ── dev tools (composer / node / bun) ────────────────────────────────────────

#[tauri::command]
pub async fn list_tools() -> Result<Response, GuiError> {
    finish(exchange(&Request::ListTools).await?)
}

#[tauri::command]
pub async fn install_tool(tool: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::InstallTool { tool }).await?)
}

#[tauri::command]
pub async fn uninstall_tool(tool: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::UninstallTool { tool }).await?)
}

#[tauri::command]
pub async fn install_tool_streamed(tool: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::InstallToolStreamed { tool }).await?)
}

// ── tunnels (Cloudflare Tunnel integration) ──────────────────────────────────

/// Install the `cloudflared` binary as a streamed job (returns a job id).
#[tauri::command]
pub async fn install_cloudflared_streamed() -> Result<Response, GuiError> {
    finish(exchange(&Request::InstallCloudflaredStreamed).await?)
}

/// Publish a site at a temporary `*.trycloudflare.com` Quick Tunnel URL.
#[tauri::command]
pub async fn start_quick_tunnel(site: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::StartQuickTunnel { site }).await?)
}

/// Tear down a site's running tunnel.
#[tauri::command]
pub async fn stop_tunnel(site: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::StopTunnel { site }).await?)
}

/// Report the live tunnels plus `cloudflared` install/login status.
#[tauri::command]
pub async fn tunnel_status() -> Result<Response, GuiError> {
    finish(exchange(&Request::TunnelStatus).await?)
}

/// Log in to a Cloudflare account as a streamed job (surfaces the auth URL).
#[tauri::command]
pub async fn cloudflared_login() -> Result<Response, GuiError> {
    finish(exchange(&Request::CloudflaredLogin).await?)
}

/// Create a named tunnel on the logged-in account.
#[tauri::command]
pub async fn create_named_tunnel(name: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::CreateNamedTunnel { name }).await?)
}

/// Delete a named tunnel from the account and forget it locally.
#[tauri::command]
pub async fn delete_named_tunnel(name: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::DeleteNamedTunnel { name }).await?)
}

/// List the locally recorded named tunnels, site mappings, and authorized zone.
#[tauri::command]
pub async fn list_named_tunnels() -> Result<Response, GuiError> {
    finish(exchange(&Request::ListNamedTunnels).await?)
}

/// Create the proxied DNS route pointing `hostname` at `tunnel`.
#[tauri::command]
pub async fn route_tunnel_dns(tunnel: String, hostname: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::RouteTunnelDns { tunnel, hostname }).await?)
}

/// Persist (or clear, with `None`) a site's public hostname mapping.
#[tauri::command]
pub async fn set_site_tunnel(site: String, hostname: Option<String>) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetSiteTunnel { site, hostname }).await?)
}

/// (Re)start the consolidated named tunnel serving every enabled site.
#[tauri::command]
pub async fn start_named_tunnel() -> Result<Response, GuiError> {
    finish(exchange(&Request::StartNamedTunnel).await?)
}

/// Stop the consolidated named tunnel.
#[tauri::command]
pub async fn stop_named_tunnel() -> Result<Response, GuiError> {
    finish(exchange(&Request::StopNamedTunnel).await?)
}

// ── site creation ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn create_site(spec: yerd_ipc::CreateSiteSpec) -> Result<Response, GuiError> {
    finish(exchange(&Request::CreateSite { spec }).await?)
}

#[tauri::command]
pub async fn job_status(job_id: String, cursor: u64) -> Result<Response, GuiError> {
    finish(exchange(&Request::JobStatus { job_id, cursor }).await?)
}

#[tauri::command]
pub async fn job_cancel(job_id: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::JobCancel { job_id }).await?)
}

// ── host helpers ───────────────────────────────────────────────────────────

/// Persist a mail attachment into the app cache and return its absolute path.
///
/// The OS opener cannot open a `data:` URL as a document, so the frontend writes
/// the decoded attachment bytes here first and then opens the returned path.
#[tauri::command]
pub async fn save_mail_attachment(
    app: tauri::AppHandle,
    filename: String,
    bytes: Vec<u8>,
) -> Result<String, GuiError> {
    let mut dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| GuiError::internal(format!("could not locate cache directory: {e}")))?;
    dir.push("mail-attachments");
    std::fs::create_dir_all(&dir)
        .map_err(|e| GuiError::internal(format!("could not create attachment cache: {e}")))?;

    let safe_name = safe_attachment_filename(&filename);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| GuiError::internal(format!("system clock error: {e}")))?
        .as_millis();
    dir.push(format!("{stamp}-{safe_name}"));

    std::fs::write(&dir, bytes)
        .map_err(|e| GuiError::internal(format!("could not write attachment: {e}")))?;
    Ok(dir.to_string_lossy().into_owned())
}

fn safe_attachment_filename(name: &str) -> String {
    let candidate = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("attachment")
        .trim();
    let filtered: String = candidate
        .chars()
        .map(|c| match c {
            ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if filtered.is_empty() {
        "attachment".to_owned()
    } else {
        filtered
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn finish_passes_success_through() {
        match finish(Response::Ok) {
            Ok(Response::Ok) => {}
            other => panic!("expected Ok(Response::Ok), got {other:?}"),
        }
        match finish(Response::Sites { sites: vec![] }) {
            Ok(Response::Sites { sites }) => assert!(sites.is_empty()),
            other => panic!("expected Sites, got {other:?}"),
        }
    }

    #[test]
    fn finish_maps_daemon_error_to_gui_error() {
        let err = finish(Response::Error {
            code: ErrorCode::NotFound,
            message: "no such site".to_owned(),
        })
        .unwrap_err();
        assert_eq!(err.code, "not_found");
        assert_eq!(err.message, "no such site");
    }

    #[test]
    fn code_str_renders_snake_case_for_every_known_variant() {
        assert_eq!(code_str(&ErrorCode::NotFound), "not_found");
        assert_eq!(code_str(&ErrorCode::AlreadyExists), "already_exists");
        assert_eq!(code_str(&ErrorCode::InvalidPath), "invalid_path");
        assert_eq!(code_str(&ErrorCode::Internal), "internal");
    }

    #[test]
    fn safe_attachment_filename_strips_path_and_unsafe_chars() {
        let cases = [
            ("invoice.pdf", "invoice.pdf"),
            ("../../etc/passwd", "passwd"),
            (r"C:\Temp\report.pdf", "report.pdf"),
            ("bad:name*.pdf", "bad_name_.pdf"),
            ("   ", "attachment"),
            ("", "attachment"),
            ("ok name.docx", "ok name.docx"),
        ];
        for (input, expected) in cases {
            assert_eq!(safe_attachment_filename(input), expected, "input={input:?}");
        }
    }
}
