//! Pure command→request mapping and response→output rendering.
//!
//! Both directions are I/O-free and unit-tested: `to_request` validates
//! arguments client-side (so a bad name/version is a clean usage error before
//! any connect), and `render` turns a [`Response`] into stdout/stderr text and
//! an exit code.

use std::fmt::Write as _;

use yerd_core::{PhpVersion, Site, SiteKind};
use yerd_ipc::{
    Channel, Diagnosis, FixReport, PhpPoolStatus, PoolRunState, PortStatus, Request, Response,
    ServiceAvailability, ServiceRunState, ServiceStatus, Severity, StatusReport, ToolStatus,
    UpdateSource,
};

use crate::cli::{Command, DbAction, MailAction, ServiceAction};
use crate::error::ClientError;

/// Map a parsed [`Command`] to the wire [`Request`], validating site names and
/// PHP versions client-side. `Use` maps to [`Request::SetPhp`].
#[allow(clippy::too_many_lines)] // one arm per command — naturally long
pub fn to_request(cmd: &Command) -> Result<Request, ClientError> {
    Ok(match cmd {
        Command::Ping => Request::Ping,
        Command::Sites => Request::ListSites,
        Command::Park { path } => Request::Park { path: path.clone() },
        Command::Link { name, path } => {
            validate_name(name)?;
            Request::Link {
                name: name.clone(),
                path: path.clone(),
            }
        }
        Command::Unlink { name } => {
            validate_name(name)?;
            Request::Unlink { name: name.clone() }
        }
        // Pure: the path is passed through as a string (like `Park`, which the
        // daemon canonicalises). For `unpark` the daemon matches the stored
        // canonical string *exactly* (no canonicalisation), so `run` best-effort
        // canonicalises this path at the I/O boundary before sending.
        Command::Unpark { path } => Request::Unpark {
            path: path.to_string_lossy().into_owned(),
        },
        // One arg = global default PHP; two args = a site's version.
        Command::Use {
            first,
            version: None,
        } => Request::SetDefaultPhp {
            version: parse_php(first)?,
        },
        Command::Use {
            first,
            version: Some(version),
        } => {
            validate_name(first)?;
            Request::SetPhp {
                name: first.clone(),
                version: parse_php(version)?,
            }
        }
        Command::Set {
            target: crate::cli::SetTarget::Php { setting, value },
        } => {
            validate_php_setting(setting, Some(value))?;
            Request::SetPhpSettings {
                settings: std::collections::BTreeMap::from([(setting.clone(), value.clone())]),
            }
        }
        Command::Unset {
            target: crate::cli::UnsetTarget::Php { setting },
        } => {
            validate_php_setting(setting, None)?;
            // Empty value is the wire convention for "remove / reset".
            Request::SetPhpSettings {
                settings: std::collections::BTreeMap::from([(setting.clone(), String::new())]),
            }
        }
        Command::Install {
            target: crate::cli::InstallTarget::Php { version },
        } => Request::InstallPhp {
            version: parse_php(version)?,
        },
        Command::Install {
            target: crate::cli::InstallTarget::Tool { id },
        } => Request::InstallTool { tool: id.clone() },
        Command::Restart {
            target: crate::cli::RestartTarget::Php { version },
        } => match version {
            Some(v) => Request::RestartPhp {
                version: parse_php(v)?,
            },
            None => Request::RestartAllPhp,
        },
        Command::Restart {
            target: crate::cli::RestartTarget::Daemon,
        } => Request::RestartDaemon,
        Command::Uninstall {
            target: Some(crate::cli::UninstallTarget::Php { version }),
            ..
        } => Request::UninstallPhp {
            version: parse_php(version)?,
        },
        Command::Uninstall {
            target: Some(crate::cli::UninstallTarget::Tool { id }),
            ..
        } => Request::UninstallTool { tool: id.clone() },
        // Bare `yerd uninstall` (no target) is the full self-uninstall, handled
        // locally in `crate::uninstall` (it tears down the daemon + files). `run`
        // branches before calling `to_request`; this arm keeps the match total.
        Command::Uninstall { target: None, .. } => {
            return Err(ClientError::Usage(
                "full uninstall is handled locally, not over IPC".to_owned(),
            ));
        }
        Command::Tools => Request::ListTools,
        Command::List {
            target: crate::cli::ListTarget::Php { check, available },
        } => {
            if *available {
                Request::AvailablePhp
            } else if *check {
                Request::CheckPhpUpdates
            } else {
                Request::ListPhp
            }
        }
        Command::List {
            target: crate::cli::ListTarget::Parked,
        } => Request::ListParked,
        Command::Update {
            target: Some(crate::cli::UpdateTarget::Php { version }),
            yes,
            edge,
            stable,
            force,
        } => {
            // clap does not structurally reject a parent flag before a
            // subcommand, so guard here: the self-update flags only apply to
            // `yerd update` (no subcommand).
            if *yes || *edge || *stable || *force {
                return Err(ClientError::Usage(
                    "--yes/--edge/--stable/--force apply to `yerd update` (no subcommand); \
                     `yerd update php` takes none of them"
                        .to_owned(),
                ));
            }
            Request::UpdatePhp {
                version: version.as_deref().map(parse_php).transpose()?,
            }
        }
        // `yerd update` (no subcommand): a Yerd self-update check. `--edge` /
        // `--stable` override the channel for this check only. The `--yes` apply
        // path is intercepted earlier in `run` (it is not a single round-trip).
        Command::Update {
            target: None,
            edge,
            stable,
            ..
        } => Request::CheckUpdate {
            channel: channel_from_flags(*edge, *stable),
        },
        Command::Services => Request::ListServices,
        Command::Service { action } => service_request(action),
        Command::Db { action } => db_request(action),
        Command::Mail { action } => match action {
            MailAction::List => Request::ListMails,
            MailAction::Show { id } => Request::GetMail { id: id.clone() },
            MailAction::Clear => Request::ClearMails,
        },
        Command::Status => Request::Status,
        Command::Doctor { action: None } => Request::Diagnose,
        Command::Doctor {
            action: Some(crate::cli::DoctorAction::Fix),
        } => Request::DoctorFix,
        Command::Secure { name } => {
            validate_name(name)?;
            Request::SetSecure {
                name: name.clone(),
                secure: true,
            }
        }
        Command::Unsecure { name } => {
            validate_name(name)?;
            Request::SetSecure {
                name: name.clone(),
                secure: false,
            }
        }
        // `--auto` (or omitting the path) resets to auto-detection (`None`).
        Command::Root { name, path, auto } => {
            validate_name(name)?;
            Request::SetWebRoot {
                name: name.clone(),
                path: if *auto { None } else { path.clone() },
            }
        }
        // `elevate`/`unelevate` are handled locally in `crate::elevate` (they
        // spawn the privileged helper), never mapped to a single IPC request.
        // `run` branches before calling `to_request`; these arms keep the match
        // total.
        Command::Elevate { .. } | Command::Unelevate { .. } => {
            return Err(ClientError::Usage(
                "elevate/unelevate are handled locally, not over IPC".to_owned(),
            ));
        }
        Command::Path { .. } => {
            return Err(ClientError::Usage(
                "path is handled locally, not over IPC".to_owned(),
            ));
        }
    })
}

/// Map a `yerd service <action>` to its wire request. Service ids are passed
/// through verbatim — the daemon returns `NotFound` for an unknown id.
fn service_request(action: &ServiceAction) -> Request {
    match action {
        ServiceAction::Available => Request::AvailableServices,
        ServiceAction::Install { service, version } => Request::InstallService {
            service: service.clone(),
            version: version.clone(),
        },
        ServiceAction::ChangeVersion { service, version } => Request::ChangeServiceVersion {
            service: service.clone(),
            version: version.clone(),
        },
        ServiceAction::Uninstall {
            service,
            version,
            purge,
        } => Request::UninstallService {
            service: service.clone(),
            version: version.clone(),
            purge: *purge,
        },
        ServiceAction::Start { service } => Request::StartService {
            service: service.clone(),
        },
        ServiceAction::Stop { service } => Request::StopService {
            service: service.clone(),
        },
        ServiceAction::Restart { service } => Request::RestartService {
            service: service.clone(),
        },
        ServiceAction::SetPort { service, port } => Request::SetServicePort {
            service: service.clone(),
            port: *port,
        },
        ServiceAction::Logs { service, lines } => Request::ServiceLogs {
            service: service.clone(),
            lines: *lines,
        },
    }
}

fn db_request(action: &DbAction) -> Request {
    match action {
        DbAction::List { service } => Request::ListDatabases {
            service: service.clone(),
        },
        DbAction::Create { service, name } => Request::CreateDatabase {
            service: service.clone(),
            name: name.clone(),
        },
        DbAction::Drop { service, name } => Request::DropDatabase {
            service: service.clone(),
            name: name.clone(),
        },
        // Paths are passed through untouched here to keep this mapping I/O-free;
        // `lib::run` absolutises them against the user's cwd at the I/O boundary.
        DbAction::Backup {
            service,
            name,
            path,
        } => Request::BackupDatabase {
            service: service.clone(),
            name: name.clone(),
            path: path.clone(),
        },
        DbAction::Restore {
            service,
            name,
            path,
        } => Request::RestoreDatabase {
            service: service.clone(),
            name: name.clone(),
            path: path.clone(),
        },
    }
}

fn parse_php(s: &str) -> Result<PhpVersion, ClientError> {
    s.parse::<PhpVersion>()
        .map_err(|e| ClientError::Usage(format!("invalid PHP version {s:?}: {e}")))
}

/// The channel override for a self-update check, from the `--edge`/`--stable`
/// flags (mutually exclusive at the clap layer). `None` = use the saved default.
#[must_use]
pub fn channel_from_flags(edge: bool, stable: bool) -> Option<Channel> {
    if edge {
        Some(Channel::Edge)
    } else if stable {
        Some(Channel::Stable)
    } else {
        None
    }
}

/// Lowercase display name for a wire channel.
fn channel_str(c: Channel) -> &'static str {
    match c {
        Channel::Edge => "edge",
        // `Channel` is `#[non_exhaustive]`; treat anything else as stable.
        _ => "stable",
    }
}

/// Render the `yerd update` report: current version, both channel latests, the
/// active channel, the availability status, and whether the figures are live or
/// cached. Both channel latests are always shown (per the feature spec).
#[allow(clippy::too_many_arguments)]
fn format_update_status(
    current: &str,
    latest_stable: Option<&str>,
    latest_edge: Option<&str>,
    channel: Channel,
    available: bool,
    target: Option<&str>,
    ahead_of_stable: bool,
    source: UpdateSource,
) -> String {
    let unknown = "unknown";
    let mut out = String::new();
    let _ = writeln!(out, "Current:       {current}");
    let _ = writeln!(out, "Latest stable: {}", latest_stable.unwrap_or(unknown));
    let _ = writeln!(out, "Latest edge:   {}", latest_edge.unwrap_or(unknown));
    let _ = writeln!(out, "Channel:       {}", channel_str(channel));
    let status = match (available, target) {
        (true, Some(t)) => format!("update available: {t}"),
        _ if ahead_of_stable => "up to date (on a pre-release ahead of stable)".to_owned(),
        _ => "up to date".to_owned(),
    };
    let _ = writeln!(out, "Status:        {status}");
    let src = match source {
        UpdateSource::Cached => "cached (offline — last known values)",
        _ => "live",
    };
    let _ = write!(out, "Source:        {src}");
    out
}

/// Validate a PHP setting name (always) and value (when setting, not unsetting)
/// client-side, so a typo is a clean usage error before connecting.
fn validate_php_setting(setting: &str, value: Option<&str>) -> Result<(), ClientError> {
    if !yerd_core::php_settings::is_supported(setting) {
        return Err(ClientError::Usage(format!(
            "unknown PHP setting {setting:?}; supported: {}",
            yerd_core::php_settings::supported_names().join(", ")
        )));
    }
    if let Some(v) = value {
        yerd_core::php_settings::validate_value(setting, v)
            .map_err(|e| ClientError::Usage(e.to_string()))?;
    }
    Ok(())
}

/// Validate a site name client-side by constructing a throwaway `Site` (the
/// document root is irrelevant — only the name is checked).
fn validate_name(name: &str) -> Result<(), ClientError> {
    Site::linked(name, "/", PhpVersion::new(8, 3))
        .map(|_| ())
        .map_err(|e| ClientError::Usage(format!("invalid site name {name:?}: {e}")))
}

/// The result of rendering a response: text to print and a process exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// Text for stdout (may be empty).
    pub stdout: String,
    /// Text for stderr (may be empty).
    pub stderr: String,
    /// Process exit code.
    pub code: u8,
}

impl Rendered {
    fn ok(stdout: String) -> Self {
        Self {
            stdout,
            stderr: String::new(),
            code: 0,
        }
    }

    fn err(stderr: String) -> Self {
        Self {
            stdout: String::new(),
            stderr,
            code: 1,
        }
    }
}

/// Render a daemon [`Response`] to stdout/stderr text + an exit code. With
/// `json`, prints the response as pretty JSON instead of a human table.
#[must_use]
pub fn render(resp: &Response, json: bool) -> Rendered {
    // Exit code is doctor-aware and computed once, so the `--json` and human
    // paths agree: `1` for an error response or any `Fail` finding, else `0`.
    let code = doctor_exit_code(resp);
    if json {
        let body = serde_json::to_string_pretty(resp)
            .unwrap_or_else(|e| format!("{{\"error\":\"serialize failed: {e}\"}}"));
        return Rendered {
            stdout: body,
            stderr: String::new(),
            code,
        };
    }
    match resp {
        Response::Pong => Rendered::ok("pong".to_owned()),
        Response::Ok => Rendered::ok("ok".to_owned()),
        Response::Sites { sites } => Rendered::ok(format_sites(sites)),
        Response::Parked { paths } => Rendered::ok(format_parked(paths)),
        Response::PhpVersions {
            installed,
            default,
            updates,
            settings,
        } => Rendered::ok(format_php_versions(installed, *default, updates, settings)),
        Response::AvailablePhp {
            available,
            installed,
        } => Rendered::ok(format_available_php(available, installed)),
        Response::Error { code: c, message } => Rendered::err(format!("error ({c:?}): {message}")),
        Response::Status { report } => Rendered {
            stdout: format_status(report),
            stderr: String::new(),
            code,
        },
        Response::Diagnoses { items } => Rendered {
            stdout: format_doctor(items),
            stderr: String::new(),
            code,
        },
        Response::DoctorFix { report } => Rendered {
            stdout: format_fix(report),
            stderr: String::new(),
            code,
        },
        Response::Services { services } => Rendered::ok(format_services(services)),
        Response::AvailableServices { services } => {
            Rendered::ok(format_available_services(services))
        }
        Response::ServiceLogs { lines } => Rendered::ok(if lines.is_empty() {
            "no log output".to_owned()
        } else {
            lines.join("\n")
        }),
        Response::Databases { databases } => Rendered::ok(if databases.is_empty() {
            "no databases".to_owned()
        } else {
            databases
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        }),
        Response::Mails { mails } => Rendered::ok(format_mails(mails)),
        Response::Mail { mail } => Rendered::ok(format_mail(mail)),
        Response::Tools { tools } => Rendered::ok(format_tools(tools)),
        Response::UpdateStatus {
            current,
            latest_stable,
            latest_edge,
            channel,
            available,
            target,
            ahead_of_stable,
            source,
        } => Rendered::ok(format_update_status(
            current,
            latest_stable.as_deref(),
            latest_edge.as_deref(),
            *channel,
            *available,
            target.as_deref(),
            *ahead_of_stable,
            *source,
        )),
        // `Response` is `#[non_exhaustive]`; a future variant from a newer
        // daemon is surfaced benignly rather than panicking.
        _ => Rendered::err("unexpected response from daemon".to_owned()),
    }
}

/// Process exit code for a response: `1` for an error or any `Fail`-severity
/// doctor finding, otherwise `0`. Pure; used by both the JSON and human paths.
#[must_use]
pub fn doctor_exit_code(resp: &Response) -> u8 {
    match resp {
        Response::Error { .. } => 1,
        Response::Diagnoses { items } => {
            u8::from(items.iter().any(|d| d.severity == Severity::Fail))
        }
        Response::DoctorFix { report } => {
            u8::from(report.manual.iter().any(|d| d.severity == Severity::Fail))
        }
        _ => 0,
    }
}

fn format_sites(sites: &[Site]) -> String {
    if sites.is_empty() {
        return "no sites".to_owned();
    }
    let mut out = String::from("NAME\tKIND\tPHP\tSECURE\tSERVED\tDOCROOT");
    for s in sites {
        let kind = match s.kind() {
            SiteKind::Parked => "parked",
            SiteKind::Linked => "linked",
        };
        // The served subdirectory (web root) relative to the document root;
        // "/" when the project root itself is served.
        let served = if s.web_subpath().as_os_str().is_empty() {
            "/".to_owned()
        } else {
            s.web_subpath().display().to_string()
        };
        let _ = write!(
            out,
            "\n{}\t{}\t{}\t{}\t{}\t{}",
            s.name(),
            kind,
            s.php(),
            s.secure(),
            served,
            s.document_root().display()
        );
    }
    out
}

fn format_parked(paths: &[String]) -> String {
    if paths.is_empty() {
        return "no parked folders".to_owned();
    }
    paths.join("\n")
}

fn format_service_state(s: ServiceRunState) -> &'static str {
    match s {
        ServiceRunState::Running => "running",
        ServiceRunState::Stopped => "stopped",
        ServiceRunState::Failed => "failed",
        // `ServiceRunState` is `#[non_exhaustive]`; a newer daemon's state reads
        // as "unknown" rather than failing the render.
        _ => "unknown",
    }
}

/// Flatten tab/CR/LF in a value so a folded or multi-line mail header can't
/// break the tab-separated `yerd mail list` table (the `--json` path needs no
/// such treatment — serde already escapes control bytes).
fn flatten_cell(s: &str) -> String {
    s.replace(['\t', '\r', '\n'], " ")
}

/// Render `yerd mail list` — a tab-separated table of captured emails.
fn format_mails(mails: &[yerd_ipc::MailSummary]) -> String {
    if mails.is_empty() {
        return "no captured emails".to_owned();
    }
    let mut out = String::from("ID\tFROM\tSUBJECT");
    for m in mails {
        let subject = if m.subject.is_empty() {
            "(no subject)".to_owned()
        } else {
            flatten_cell(&m.subject)
        };
        let _ = write!(out, "\n{}\t{}\t{}", m.id, flatten_cell(&m.from), subject);
    }
    out
}

/// Render `yerd mail show <id>` — headers followed by the text body (falling
/// back to a note when only an HTML body is present).
fn format_mail(mail: &yerd_ipc::MailDetail) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "From:    {}", mail.from);
    let _ = writeln!(out, "To:      {}", mail.to.join(", "));
    let _ = writeln!(out, "Subject: {}", mail.subject);
    out.push('\n');
    match (&mail.text_body, &mail.html_body) {
        (Some(text), _) => out.push_str(text),
        (None, Some(_)) => out.push_str("(HTML-only message — open it in the GUI viewer)"),
        (None, None) => out.push_str("(empty message)"),
    }
    out
}

fn format_services(services: &[ServiceStatus]) -> String {
    if services.is_empty() {
        return "no services".to_owned();
    }
    let mut out = String::from("SERVICE\tSTATE\tPORT\tVERSION\tENABLED\tINSTALLED");
    for s in services {
        // Not installed: there's no live state, port, or version to report.
        if s.installed_versions.is_empty() {
            let _ = write!(
                out,
                "\n{}\tnot installed\t-\t-\t{}\t-",
                s.service, s.enabled
            );
            continue;
        }
        // The active/selected version, falling back to the latest on disk.
        let version = s
            .selected_version
            .as_deref()
            .or_else(|| s.installed_versions.last().map(String::as_str))
            .unwrap_or("-");
        let _ = write!(
            out,
            "\n{}\t{}\t{}\t{}\t{}\t{}",
            s.service,
            format_service_state(s.state),
            s.port,
            version,
            s.enabled,
            s.installed_versions.join(",")
        );
    }
    out
}

/// Render `yerd tools` as a tab-separated table (tool, status, commands).
fn format_tools(tools: &[ToolStatus]) -> String {
    if tools.is_empty() {
        return "no tools".to_owned();
    }
    let mut out = String::from("TOOL\tSTATUS\tCOMMANDS");
    for t in tools {
        let status = if t.installed {
            t.version.as_deref().unwrap_or("installed")
        } else if t.external {
            "external"
        } else {
            "not installed"
        };
        let _ = write!(out, "\n{}\t{}\t{}", t.id, status, t.binaries.join(","));
    }
    out
}

fn format_available_services(services: &[ServiceAvailability]) -> String {
    if services.is_empty() {
        return "no services available".to_owned();
    }
    let mut out = String::from("SERVICE\tAVAILABLE\tINSTALLED");
    for s in services {
        let available = if s.available.is_empty() {
            "-".to_owned()
        } else {
            s.available.join(",")
        };
        let installed = if s.installed.is_empty() {
            "-".to_owned()
        } else {
            s.installed.join(",")
        };
        let _ = write!(out, "\n{}\t{}\t{}", s.service, available, installed);
    }
    out
}

fn format_php_versions(
    installed: &[PhpVersion],
    default: PhpVersion,
    updates: &[yerd_ipc::PhpUpdate],
    settings: &std::collections::BTreeMap<String, String>,
) -> String {
    let versions = if installed.is_empty() {
        format!("no PHP versions installed (default: {default}) — `yerd install php {default}`")
    } else {
        installed
            .iter()
            .map(|v| {
                let mut line = if *v == default {
                    format!("{v} (default)")
                } else {
                    v.to_string()
                };
                if let Some(u) = updates.iter().find(|u| u.version == *v) {
                    let _ = write!(line, " — update available: {} → {}", u.installed, u.latest);
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    if settings.is_empty() {
        return versions;
    }
    let mut out = versions;
    out.push_str("\n\nsettings:");
    for (k, v) in settings {
        let _ = write!(out, "\n  {k} = {v}");
    }
    out
}

/// Render the installable versions, tagging the ones already installed.
fn format_available_php(available: &[PhpVersion], installed: &[PhpVersion]) -> String {
    if available.is_empty() {
        return "no installable PHP versions found for this platform".to_owned();
    }
    available
        .iter()
        .map(|v| {
            if installed.contains(v) {
                format!("{v} (installed)")
            } else {
                v.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a [`StatusReport`] as a human-readable block.
fn format_status(r: &StatusReport) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    // The proxy + DNS run inside the daemon process, so its RSS covers them.
    let rss = r
        .daemon_rss_bytes
        .map(|b| format!(", rss {}", fmt_bytes(b)))
        .unwrap_or_default();
    let _ = writeln!(
        s,
        "daemon    running (pid {}, up {}{})",
        r.daemon_pid,
        fmt_duration(r.uptime_secs),
        rss
    );
    // Empty when talking to a daemon predating version reporting (the field is
    // `#[serde(default)]`); show "unknown" rather than a blank value.
    let version = if r.daemon_version.is_empty() {
        "unknown"
    } else {
        &r.daemon_version
    };
    let _ = writeln!(s, "version   {version}");
    let _ = writeln!(s, "tld       .{}", r.tld);
    let redirected = r.port_redirect == Some(true);
    if let Some(u) = r.web_unbound {
        // Degraded: bound nothing, so `fmt_port` would print a misleading
        // "80 → 0 (fallback)". Surface the real state instead.
        let _ = writeln!(
            s,
            "http      not serving — couldn't bind {} (run `yerd doctor`)",
            u.http
        );
        let _ = writeln!(s, "https     not serving — couldn't bind {}", u.https);
    } else {
        let _ = writeln!(s, "http      {}", fmt_port(r.http, redirected));
        let _ = writeln!(s, "https     {}", fmt_port(r.https, redirected));
    }
    if r.foreign_web_listener == Some(true) {
        let _ = writeln!(
            s,
            "ports     conflict: another process is using 80/443 (run `yerd doctor`)"
        );
    }
    if let Some(port) = r.dns_unbound {
        // Degraded: `dns_addr` holds the *wanted* address, so printing it would
        // read as healthy. Surface the real state instead.
        let _ = writeln!(
            s,
            "dns       not resolving — couldn't bind port {port} (run `yerd doctor`)"
        );
    } else {
        let _ = writeln!(s, "dns       {}", r.dns_addr);
    }
    let _ = writeln!(
        s,
        "ca        trusted: {}  ({})",
        fmt_tristate(r.ca.trusted_system),
        r.ca.path.display()
    );
    let _ = writeln!(
        s,
        "resolver  installed: {}",
        fmt_tristate(r.resolver_installed)
    );
    // Resolver off: `.test` names don't resolve, so point the user at the
    // localhost fallback (the proxy serves any site at /~<name>.<tld>). Skip it
    // when degraded (`web_unbound`): there's no bound web port to reach, so
    // `r.http.bound` would be a misleading 0.
    if r.resolver_installed == Some(false) && r.web_unbound.is_none() {
        // Omit the port when it's the default 80 (matches the GUI's URL math).
        let port = if r.http.bound == 80 {
            String::new()
        } else {
            format!(":{}", r.http.bound)
        };
        let _ = writeln!(
            s,
            "          → not installed: reach sites at http://localhost{port}/~<name>.{}",
            r.tld
        );
    }
    if let Some([one, five, fifteen]) = r.load_avg {
        let _ = writeln!(
            s,
            "load      {} {} {}",
            fmt_centi(one),
            fmt_centi(five),
            fmt_centi(fifteen)
        );
    }
    let _ = writeln!(
        s,
        "sites     {} parked, {} linked, {} secured",
        r.sites.parked, r.sites.linked, r.sites.secured
    );

    if r.php.is_empty() {
        let _ = write!(s, "\nphp       none installed");
        return s;
    }
    let _ = write!(s, "\nphp");
    for p in &r.php {
        let _ = write!(s, "{}", format_php_pool_line(p, r.default_php));
    }
    s
}

/// Render one PHP pool's status line (leading `"\n  "`), used by [`format_status`].
fn format_php_pool_line(p: &PhpPoolStatus, default_php: PhpVersion) -> String {
    use std::fmt::Write;
    let default = if p.version == default_php {
        " (default)"
    } else {
        ""
    };
    let state = match p.state {
        PoolRunState::Running => "running",
        PoolRunState::Stopped => "stopped",
        PoolRunState::Failed => "failed",
        _ => "?",
    };
    let mut line = format!("\n  {}{default}  {state}", p.version);
    if let Some(pid) = p.pid {
        let _ = write!(line, "  pid {pid}");
    }
    if let Some(listen) = &p.listen {
        let _ = write!(line, "  {listen}");
    }
    if let Some(rss) = p.rss_bytes {
        let _ = write!(line, "  rss {}", fmt_bytes(rss));
    }
    if let Some(update) = &p.update_available {
        let _ = write!(line, "  update→{update}");
    }
    line
}

/// Render the doctor findings as ✓/⚠/✗ lines with remedies.
fn format_doctor(items: &[Diagnosis]) -> String {
    use std::fmt::Write;
    if items.is_empty() {
        return "no findings".to_owned();
    }
    let mut s = String::new();
    for (i, d) in items.iter().enumerate() {
        if i > 0 {
            s.push('\n');
        }
        let _ = write!(s, "{} {}", severity_mark(d.severity), d.title);
        if !d.detail.is_empty() {
            let _ = write!(s, "\n    {}", d.detail);
        }
        if let Some(remedy) = &d.remedy {
            let _ = write!(s, "\n    → {remedy}");
        }
    }
    s
}

/// Render a [`FixReport`]: what was fixed, then what still needs attention.
fn format_fix(report: &FixReport) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    if report.performed.is_empty() {
        s.push_str("no automatic fixes were applicable");
    } else {
        s.push_str("applied fixes:");
        for f in &report.performed {
            let mark = if f.ok { "✓" } else { "✗" };
            let _ = write!(s, "\n  {mark} {}", f.message);
        }
    }
    if !report.manual.is_empty() {
        s.push_str("\n\nstill needs attention:");
        for d in &report.manual {
            let _ = write!(s, "\n  {} {}", severity_mark(d.severity), d.title);
            if let Some(remedy) = &d.remedy {
                let _ = write!(s, "\n      → {remedy}");
            }
        }
    }
    s
}

fn severity_mark(sev: Severity) -> &'static str {
    match sev {
        Severity::Ok => "✓",
        Severity::Warn => "⚠",
        Severity::Fail => "✗",
        _ => "•",
    }
}

fn fmt_port(p: PortStatus, redirected: bool) -> String {
    if p.fell_back {
        // The listener bound a rootless port, but on macOS an active pf redirect
        // (`redirected`) carries the privileged port to it — so it's reachable on
        // the requested port, not merely "fallen back".
        let tag = if redirected { "redirected" } else { "fallback" };
        format!("{} → {} ({tag})", p.requested, p.bound)
    } else {
        p.bound.to_string()
    }
}

fn fmt_tristate(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

/// Render integer hundredths (e.g. `152`) as a decimal (`1.52`).
fn fmt_centi(c: u32) -> String {
    format!("{}.{:02}", c / 100, c % 100)
}

/// Human-readable uptime, coarse-grained.
fn fmt_duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h{m}m")
    } else if m > 0 {
        format!("{m}m{s}s")
    } else {
        format!("{s}s")
    }
}

/// Human-readable byte size (integer math; no float-cast lints).
fn fmt_bytes(b: u64) -> String {
    if b < 1024 {
        return format!("{b} B");
    }
    let kib = b / 1024;
    if kib < 1024 {
        return format!("{kib} KiB");
    }
    let mib_whole = kib / 1024;
    let mib_tenths = (kib % 1024) * 10 / 1024;
    format!("{mib_whole}.{mib_tenths} MiB")
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
    use std::path::PathBuf;
    use yerd_ipc::ErrorCode;

    #[test]
    #[allow(clippy::too_many_lines)] // one assertion per command — naturally long
    fn maps_each_command_to_its_request() {
        assert_eq!(to_request(&Command::Ping).unwrap(), Request::Ping);
        assert_eq!(to_request(&Command::Sites).unwrap(), Request::ListSites);
        assert_eq!(
            to_request(&Command::Park {
                path: PathBuf::from("/srv/sites")
            })
            .unwrap(),
            Request::Park {
                path: PathBuf::from("/srv/sites")
            }
        );
        assert_eq!(
            to_request(&Command::Link {
                name: "foo".into(),
                path: PathBuf::from("/srv/foo")
            })
            .unwrap(),
            Request::Link {
                name: "foo".into(),
                path: PathBuf::from("/srv/foo")
            }
        );
        assert_eq!(
            to_request(&Command::Unlink { name: "foo".into() }).unwrap(),
            Request::Unlink { name: "foo".into() }
        );
        // `unpark <path>` maps to Unpark with the path as a string (pure; the
        // I/O-boundary canonicalisation in `run` is tested separately).
        assert_eq!(
            to_request(&Command::Unpark {
                path: PathBuf::from("/srv/sites")
            })
            .unwrap(),
            Request::Unpark {
                path: "/srv/sites".into()
            }
        );
        // `list parked` maps to ListParked.
        assert_eq!(
            to_request(&Command::List {
                target: crate::cli::ListTarget::Parked
            })
            .unwrap(),
            Request::ListParked
        );
        // `use <site> <ver>` (two args) maps to SetPhp.
        assert_eq!(
            to_request(&Command::Use {
                first: "foo".into(),
                version: Some("8.4".into())
            })
            .unwrap(),
            Request::SetPhp {
                name: "foo".into(),
                version: PhpVersion::new(8, 4)
            }
        );
        // `use <ver>` (one arg) maps to the global SetDefaultPhp.
        assert_eq!(
            to_request(&Command::Use {
                first: "8.5".into(),
                version: None
            })
            .unwrap(),
            Request::SetDefaultPhp {
                version: PhpVersion::new(8, 5)
            }
        );
        // `set php <k> <v>` and `unset php <k>`.
        assert_eq!(
            to_request(&Command::Set {
                target: crate::cli::SetTarget::Php {
                    setting: "memory_limit".into(),
                    value: "512M".into()
                }
            })
            .unwrap(),
            Request::SetPhpSettings {
                settings: std::collections::BTreeMap::from([(
                    "memory_limit".to_string(),
                    "512M".to_string()
                )])
            }
        );
        assert_eq!(
            to_request(&Command::Unset {
                target: crate::cli::UnsetTarget::Php {
                    setting: "memory_limit".into()
                }
            })
            .unwrap(),
            Request::SetPhpSettings {
                settings: std::collections::BTreeMap::from([(
                    "memory_limit".to_string(),
                    String::new()
                )])
            }
        );
        // `install php <ver>` and `list php`.
        assert_eq!(
            to_request(&Command::Install {
                target: crate::cli::InstallTarget::Php {
                    version: "8.5".into()
                }
            })
            .unwrap(),
            Request::InstallPhp {
                version: PhpVersion::new(8, 5)
            }
        );
        // `restart php <ver>` / `restart php` (all) and `uninstall php <ver>`.
        assert_eq!(
            to_request(&Command::Restart {
                target: crate::cli::RestartTarget::Php {
                    version: Some("8.5".into())
                }
            })
            .unwrap(),
            Request::RestartPhp {
                version: PhpVersion::new(8, 5)
            }
        );
        assert_eq!(
            to_request(&Command::Restart {
                target: crate::cli::RestartTarget::Php { version: None }
            })
            .unwrap(),
            Request::RestartAllPhp
        );
        assert_eq!(
            to_request(&Command::Restart {
                target: crate::cli::RestartTarget::Daemon
            })
            .unwrap(),
            Request::RestartDaemon
        );
        assert_eq!(
            to_request(&Command::Uninstall {
                target: Some(crate::cli::UninstallTarget::Php {
                    version: "8.5".into()
                }),
                yes: false
            })
            .unwrap(),
            Request::UninstallPhp {
                version: PhpVersion::new(8, 5)
            }
        );
        assert_eq!(
            to_request(&Command::List {
                target: crate::cli::ListTarget::Php {
                    check: false,
                    available: false
                }
            })
            .unwrap(),
            Request::ListPhp
        );
        // `tools` / `install tool <id>` / `uninstall tool <id>`.
        assert_eq!(to_request(&Command::Tools).unwrap(), Request::ListTools);
        assert_eq!(
            to_request(&Command::Install {
                target: crate::cli::InstallTarget::Tool { id: "node".into() }
            })
            .unwrap(),
            Request::InstallTool {
                tool: "node".into()
            }
        );
        assert_eq!(
            to_request(&Command::Uninstall {
                target: Some(crate::cli::UninstallTarget::Tool { id: "bun".into() }),
                yes: false
            })
            .unwrap(),
            Request::UninstallTool { tool: "bun".into() }
        );
        assert_eq!(
            to_request(&Command::List {
                target: crate::cli::ListTarget::Php {
                    check: true,
                    available: false
                }
            })
            .unwrap(),
            Request::CheckPhpUpdates
        );
        assert_eq!(
            to_request(&Command::List {
                target: crate::cli::ListTarget::Php {
                    check: false,
                    available: true
                }
            })
            .unwrap(),
            Request::AvailablePhp
        );
        // `--available` wins over `--check`.
        assert_eq!(
            to_request(&Command::List {
                target: crate::cli::ListTarget::Php {
                    check: true,
                    available: true
                }
            })
            .unwrap(),
            Request::AvailablePhp
        );
        // `update php` / `update php <ver>`.
        assert_eq!(
            to_request(&Command::Update {
                target: Some(crate::cli::UpdateTarget::Php { version: None }),
                yes: false,
                edge: false,
                stable: false,
                force: false,
            })
            .unwrap(),
            Request::UpdatePhp { version: None }
        );
        assert_eq!(
            to_request(&Command::Update {
                target: Some(crate::cli::UpdateTarget::Php {
                    version: Some("8.5".into())
                }),
                yes: false,
                edge: false,
                stable: false,
                force: false,
            })
            .unwrap(),
            Request::UpdatePhp {
                version: Some(PhpVersion::new(8, 5))
            }
        );
        // Bare `yerd update` → CheckUpdate (no channel override).
        assert_eq!(
            to_request(&Command::Update {
                target: None,
                yes: false,
                edge: false,
                stable: false,
                force: false,
            })
            .unwrap(),
            Request::CheckUpdate { channel: None }
        );
        // `yerd update --edge` → CheckUpdate on the edge channel.
        assert_eq!(
            to_request(&Command::Update {
                target: None,
                yes: false,
                edge: true,
                stable: false,
                force: false,
            })
            .unwrap(),
            Request::CheckUpdate {
                channel: Some(Channel::Edge)
            }
        );
        // Self-update flags alongside the `php` subcommand are a usage error.
        assert!(matches!(
            to_request(&Command::Update {
                target: Some(crate::cli::UpdateTarget::Php { version: None }),
                yes: true,
                edge: false,
                stable: false,
                force: false,
            }),
            Err(ClientError::Usage(_))
        ));
        // `secure`/`unsecure` map to SetSecure with the matching flag.
        assert_eq!(
            to_request(&Command::Secure { name: "foo".into() }).unwrap(),
            Request::SetSecure {
                name: "foo".into(),
                secure: true
            }
        );
        assert_eq!(
            to_request(&Command::Unsecure { name: "foo".into() }).unwrap(),
            Request::SetSecure {
                name: "foo".into(),
                secure: false
            }
        );
        // `root <site> <path>` maps to SetWebRoot with the path.
        assert_eq!(
            to_request(&Command::Root {
                name: "foo".into(),
                path: Some("public".into()),
                auto: false,
            })
            .unwrap(),
            Request::SetWebRoot {
                name: "foo".into(),
                path: Some("public".into()),
            }
        );
        // `root <site> --auto` (and bare `root <site>`) reset to auto-detect.
        assert_eq!(
            to_request(&Command::Root {
                name: "foo".into(),
                path: Some("public".into()),
                auto: true,
            })
            .unwrap(),
            Request::SetWebRoot {
                name: "foo".into(),
                path: None,
            }
        );
        assert_eq!(
            to_request(&Command::Root {
                name: "foo".into(),
                path: None,
                auto: false,
            })
            .unwrap(),
            Request::SetWebRoot {
                name: "foo".into(),
                path: None,
            }
        );
    }

    #[test]
    fn rejects_bad_version_and_name_before_connect() {
        match to_request(&Command::Use {
            first: "foo".into(),
            version: Some("not-a-version".into()),
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
        // One-arg `use <not-a-version>` (global) must also be a usage error.
        match to_request(&Command::Use {
            first: "not-a-version".into(),
            version: None,
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
        match to_request(&Command::Link {
            name: "bad name".into(),
            path: PathBuf::from("/x"),
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
        match to_request(&Command::Unlink {
            name: "bad/name".into(),
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
        match to_request(&Command::Secure {
            name: "bad name".into(),
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
        // Unknown setting name and bad value are rejected client-side.
        match to_request(&Command::Set {
            target: crate::cli::SetTarget::Php {
                setting: "not_a_setting".into(),
                value: "1".into(),
            },
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
        match to_request(&Command::Set {
            target: crate::cli::SetTarget::Php {
                setting: "memory_limit".into(),
                value: "bogus".into(),
            },
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
    }

    #[test]
    fn renders_update_status_with_all_rows() {
        let resp = Response::UpdateStatus {
            current: "2.0.0".into(),
            latest_stable: Some("2.0.5".into()),
            latest_edge: Some("2.1.0-rc.1".into()),
            channel: Channel::Stable,
            available: true,
            target: Some("2.0.5".into()),
            ahead_of_stable: false,
            source: UpdateSource::Live,
        };
        let out = render(&resp, false).stdout;
        // Every row present, both channel latests shown, plus status + source.
        assert!(out.contains("Current:       2.0.0"), "{out}");
        assert!(out.contains("Latest stable: 2.0.5"), "{out}");
        assert!(out.contains("Latest edge:   2.1.0-rc.1"), "{out}");
        assert!(out.contains("Channel:       stable"), "{out}");
        assert!(
            out.contains("Status:        update available: 2.0.5"),
            "{out}"
        );
        assert!(out.contains("Source:        live"), "{out}");
        assert_eq!(render(&resp, false).code, 0);
    }

    #[test]
    fn renders_update_status_cached_and_ahead_of_stable() {
        // On a pre-release ahead of stable, offline: "up to date" + cached note.
        let resp = Response::UpdateStatus {
            current: "2.1.0-rc.3".into(),
            latest_stable: Some("2.0.5".into()),
            latest_edge: Some("2.1.0-rc.3".into()),
            channel: Channel::Stable,
            available: false,
            target: None,
            ahead_of_stable: true,
            source: UpdateSource::Cached,
        };
        let out = render(&resp, false).stdout;
        assert!(out.contains("ahead of stable"), "{out}");
        assert!(out.contains("Source:        cached"), "{out}");
    }

    #[test]
    fn renders_human_responses_and_exit_codes() {
        assert_eq!(render(&Response::Pong, false).stdout, "pong");
        assert_eq!(render(&Response::Pong, false).code, 0);
        assert_eq!(render(&Response::Ok, false).code, 0);

        let empty = render(&Response::Sites { sites: vec![] }, false);
        assert_eq!(empty.stdout, "no sites");
        assert_eq!(empty.code, 0);

        // `yerd tools` renders a table, not the "unexpected response" fallback.
        let tools = render(
            &Response::Tools {
                tools: vec![ToolStatus {
                    id: "node".into(),
                    display_name: "Node.js".into(),
                    installed: true,
                    version: Some("v24.17.0".into()),
                    binaries: vec!["node".into(), "npm".into(), "npx".into()],
                    external: false,
                }],
            },
            false,
        );
        assert!(tools.stdout.contains("node"));
        assert!(tools.stdout.contains("v24.17.0"));
        assert!(tools.stdout.contains("npm"));
        assert_eq!(tools.code, 0);

        let site = Site::linked("foo", "/srv/foo", PhpVersion::new(8, 3)).unwrap();
        let listed = render(&Response::Sites { sites: vec![site] }, false);
        assert!(listed.stdout.contains("foo"));
        assert!(listed.stdout.contains("linked"));
        assert!(listed.stdout.contains("8.3"));
        assert_eq!(listed.code, 0);

        let err = render(
            &Response::Error {
                code: ErrorCode::NotFound,
                message: "nope".into(),
            },
            false,
        );
        assert!(err.stdout.is_empty());
        assert!(err.stderr.contains("nope"));
        assert_eq!(err.code, 1);
    }

    #[test]
    fn renders_parked_folders() {
        let empty = render(&Response::Parked { paths: vec![] }, false);
        assert_eq!(empty.stdout, "no parked folders");
        assert_eq!(empty.code, 0);

        let listed = render(
            &Response::Parked {
                paths: vec!["/srv/a".into(), "/srv/b".into()],
            },
            false,
        );
        assert!(listed.stdout.contains("/srv/a"));
        assert!(listed.stdout.contains("/srv/b"));
        assert_eq!(listed.code, 0);
    }

    #[test]
    fn renders_php_versions_marking_default() {
        let r = render(
            &Response::PhpVersions {
                installed: vec![PhpVersion::new(8, 3), PhpVersion::new(8, 5)],
                default: PhpVersion::new(8, 5),
                updates: vec![yerd_ipc::PhpUpdate {
                    version: PhpVersion::new(8, 3),
                    installed: "8.3.6".into(),
                    latest: "8.3.31".into(),
                }],
                settings: std::collections::BTreeMap::new(),
            },
            false,
        );
        assert_eq!(r.code, 0);
        assert!(r.stdout.contains("8.5 (default)"));
        assert!(!r.stdout.contains("8.3 (default)"));
        // The 8.3 line carries the update annotation; 8.5 does not.
        assert!(r.stdout.contains("8.3 — update available: 8.3.6 → 8.3.31"));
        assert!(!r.stdout.contains("8.5 — update available"));
        // No settings → no settings block.
        assert!(!r.stdout.contains("settings:"));

        let empty = render(
            &Response::PhpVersions {
                installed: vec![],
                default: PhpVersion::new(8, 3),
                updates: vec![],
                settings: std::collections::BTreeMap::new(),
            },
            false,
        );
        assert!(empty.stdout.contains("no PHP versions installed"));
    }

    #[test]
    fn renders_php_settings_block() {
        let r = render(
            &Response::PhpVersions {
                installed: vec![PhpVersion::new(8, 5)],
                default: PhpVersion::new(8, 5),
                updates: vec![],
                settings: std::collections::BTreeMap::from([
                    ("memory_limit".to_string(), "512M".to_string()),
                    ("display_errors".to_string(), "On".to_string()),
                ]),
            },
            false,
        );
        assert_eq!(r.code, 0);
        assert!(r.stdout.contains("settings:"));
        assert!(r.stdout.contains("memory_limit = 512M"));
        assert!(r.stdout.contains("display_errors = On"));
    }

    #[test]
    fn renders_available_php_tagging_installed() {
        let r = render(
            &Response::AvailablePhp {
                available: vec![
                    PhpVersion::new(8, 3),
                    PhpVersion::new(8, 4),
                    PhpVersion::new(8, 5),
                ],
                installed: vec![PhpVersion::new(8, 4)],
            },
            false,
        );
        assert_eq!(r.code, 0);
        assert!(r.stdout.contains("8.4 (installed)"));
        // Not-installed versions appear bare (no tag).
        assert!(r.stdout.contains("\n8.3") || r.stdout.starts_with("8.3"));
        assert!(!r.stdout.contains("8.3 (installed)"));
        assert!(!r.stdout.contains("8.5 (installed)"));

        let empty = render(
            &Response::AvailablePhp {
                available: vec![],
                installed: vec![],
            },
            false,
        );
        assert!(empty.stdout.contains("no installable PHP versions"));
    }

    #[test]
    fn maps_status_and_doctor_commands() {
        assert_eq!(to_request(&Command::Status).unwrap(), Request::Status);
        assert_eq!(
            to_request(&Command::Doctor { action: None }).unwrap(),
            Request::Diagnose
        );
        assert_eq!(
            to_request(&Command::Doctor {
                action: Some(crate::cli::DoctorAction::Fix)
            })
            .unwrap(),
            Request::DoctorFix
        );
    }

    fn sample_report() -> yerd_ipc::StatusReport {
        yerd_ipc::StatusReport {
            daemon_pid: 4242,
            uptime_secs: 65,
            daemon_rss_bytes: Some(12_000_000),
            tld: "test".into(),
            http: PortStatus {
                requested: 80,
                bound: 8080,
                fell_back: true,
            },
            https: PortStatus {
                requested: 443,
                bound: 443,
                fell_back: false,
            },
            dns_addr: "127.0.0.1:1053".parse().unwrap(),
            ca: yerd_ipc::CaStatus {
                path: "/x/ca.cert.pem".into(),
                fingerprint: "ab".repeat(32),
                trusted_system: Some(false),
            },
            resolver_installed: None,
            port_redirect: None,
            foreign_web_listener: None,
            resolver_backup: None,
            default_php: PhpVersion::new(8, 5),
            php: vec![yerd_ipc::PhpPoolStatus {
                version: PhpVersion::new(8, 5),
                installed_patch: Some("8.5.6".into()),
                state: PoolRunState::Running,
                pid: Some(99),
                listen: Some("/run/fpm.sock".into()),
                rss_bytes: Some(3_200_000),
                update_available: None,
            }],
            sites: yerd_ipc::SiteCounts {
                parked: 2,
                linked: 1,
                secured: 1,
            },
            load_avg: Some([152, 48, 5]),
            daemon_version: "2.0.1".into(),
            services: vec![],
            mail: None,
            web_unbound: None,
            dns_unbound: None,
            boot_id: None,
        }
    }

    #[test]
    fn fmt_port_distinguishes_fallback_from_redirect() {
        let fell_back = PortStatus {
            requested: 80,
            bound: 8080,
            fell_back: true,
        };
        // No redirect: a plain rootless fallback.
        assert_eq!(fmt_port(fell_back, false), "80 → 8080 (fallback)");
        // pf redirect active: reachable on :80, just internally on :8080.
        assert_eq!(fmt_port(fell_back, true), "80 → 8080 (redirected)");
        // Bound directly: redirect flag is irrelevant.
        let bound = PortStatus {
            requested: 80,
            bound: 80,
            fell_back: false,
        };
        assert_eq!(fmt_port(bound, true), "80");
    }

    #[test]
    fn status_degraded_web_ports_shows_not_serving() {
        let mut r = sample_report();
        r.http = PortStatus {
            requested: 80,
            bound: 0,
            fell_back: true,
        };
        r.https = PortStatus {
            requested: 443,
            bound: 0,
            fell_back: true,
        };
        r.web_unbound = Some(yerd_ipc::UnboundWeb {
            http: 8080,
            https: 8443,
        });
        let out = format_status(&r);
        assert!(out.contains("not serving — couldn't bind 8080"), "{out}");
        assert!(out.contains("not serving — couldn't bind 8443"), "{out}");
        // The misleading "→ 0" fallback rendering must NOT appear.
        assert!(!out.contains("→ 0"), "{out}");
    }

    #[test]
    fn status_degraded_dns_shows_not_resolving() {
        let mut r = sample_report();
        r.dns_unbound = Some(1053);
        let out = format_status(&r);
        assert!(
            out.contains("not resolving — couldn't bind port 1053"),
            "{out}"
        );
        // The healthy "dns       127.0.0.1:1053" line must NOT appear.
        assert!(!out.contains("dns       127.0.0.1:1053"), "{out}");
    }

    #[test]
    fn renders_status_human_block() {
        let out = render(
            &Response::Status {
                report: Box::new(sample_report()),
            },
            false,
        );
        assert_eq!(out.code, 0);
        assert!(out.stdout.contains("pid 4242"));
        assert!(out.stdout.contains("version   2.0.1"));
        assert!(out.stdout.contains("80 → 8080 (fallback)"));
        assert!(out.stdout.contains("trusted: no"));
        assert!(out.stdout.contains("installed: unknown")); // resolver None
        assert!(out.stdout.contains("1.52 0.48 0.05")); // load hundredths → x.xx
        assert!(out.stdout.contains("8.5 (default)  running"));
        assert!(out.stdout.contains("pid 99"));
    }

    #[test]
    fn status_shows_unknown_for_empty_daemon_version() {
        // An older daemon predating version reporting sends `""` (serde default).
        let mut report = sample_report();
        report.daemon_version = String::new();
        let out = render(
            &Response::Status {
                report: Box::new(report),
            },
            false,
        );
        assert!(
            out.stdout.contains("version   unknown"),
            "got: {}",
            out.stdout
        );
    }

    #[test]
    fn renders_doctor_and_sets_exit_code_on_fail() {
        let warn_only = Response::Diagnoses {
            items: vec![Diagnosis {
                code: yerd_ipc::DiagnosisCode::CaNotTrusted,
                severity: Severity::Warn,
                title: "Local CA not trusted".into(),
                detail: "d".into(),
                remedy: Some("sudo yerd elevate trust".into()),
            }],
        };
        let r = render(&warn_only, false);
        assert_eq!(r.code, 0, "warn-only must not fail the exit code");
        assert!(r.stdout.contains("⚠ Local CA not trusted"));
        assert!(r.stdout.contains("→ sudo yerd elevate trust"));

        let with_fail = Response::Diagnoses {
            items: vec![Diagnosis {
                code: yerd_ipc::DiagnosisCode::NoPhpInstalled,
                severity: Severity::Fail,
                title: "No PHP".into(),
                detail: "d".into(),
                remedy: None,
            }],
        };
        assert_eq!(render(&with_fail, false).code, 1);
        // The JSON path must agree on the exit code.
        assert_eq!(render(&with_fail, true).code, 1);
    }

    #[test]
    fn renders_doctor_fix_report() {
        let resp = Response::DoctorFix {
            report: FixReport {
                performed: vec![yerd_ipc::FixResult {
                    code: yerd_ipc::DiagnosisCode::FpmPoolFailed,
                    ok: true,
                    message: "restarted PHP 8.5 FPM pool".into(),
                }],
                manual: vec![Diagnosis {
                    code: yerd_ipc::DiagnosisCode::ResolverNotInstalled,
                    severity: Severity::Warn,
                    title: "Resolver not installed".into(),
                    detail: "d".into(),
                    remedy: Some("sudo yerd elevate resolver".into()),
                }],
            },
        };
        let r = render(&resp, false);
        assert_eq!(r.code, 0); // only a Warn remains
        assert!(r.stdout.contains("✓ restarted PHP 8.5 FPM pool"));
        assert!(r.stdout.contains("still needs attention"));
        assert!(r.stdout.contains("sudo yerd elevate resolver"));
    }

    #[test]
    fn fmt_bytes_is_human_readable() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(2048), "2 KiB");
        assert_eq!(fmt_bytes(3_200_000), "3.0 MiB");
    }

    #[test]
    fn json_rendering_is_valid_and_codes_match() {
        let ok = render(&Response::Ok, true);
        assert!(serde_json::from_str::<serde_json::Value>(&ok.stdout).is_ok());
        assert_eq!(ok.code, 0);

        let err = render(
            &Response::Error {
                code: ErrorCode::Internal,
                message: "boom".into(),
            },
            true,
        );
        let v: serde_json::Value = serde_json::from_str(&err.stdout).unwrap();
        assert_eq!(v["type"], "error");
        assert_eq!(err.code, 1);
    }
}
