//! Pure command→request mapping and response→output rendering.
//!
//! Both directions are I/O-free and unit-tested: `to_request` validates
//! arguments client-side (so a bad name/version is a clean usage error before
//! any connect), and `render` turns a [`Response`] into stdout/stderr text and
//! an exit code.

use std::fmt::Write as _;

use yerd_core::{PhpVersion, Site, SiteKind};
use yerd_ipc::{
    Channel, CloudflaredStatus, Diagnosis, FixReport, PhpPoolStatus, PoolRunState, PortStatus,
    Request, Response, ServiceAvailability, ServiceRunState, ServiceStatus, Severity, SiteEntry,
    StatusReport, ToolStatus, TunnelInfo, TunnelRunState, UpdateSource,
};

use crate::cli::{Command, DbAction, MailAction, ServiceAction, TunnelAction};
use crate::error::ClientError;

/// Map a parsed [`Command`] to the wire [`Request`], validating site names and
/// PHP versions client-side.
#[allow(clippy::too_many_lines)]
pub fn to_request(cmd: &Command) -> Result<Request, ClientError> {
    Ok(match cmd {
        Command::Ping => Request::Ping,
        Command::Sites => Request::ListSites,
        Command::Park { path } => Request::Park { path: path.clone() },
        Command::Unlink { name } => {
            validate_name(name)?;
            Request::Unlink { name: name.clone() }
        }
        Command::Unpark { path } => Request::Unpark {
            path: path.to_string_lossy().into_owned(),
        },
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
            Request::SetPhpSettings {
                settings: std::collections::BTreeMap::from([(setting.clone(), String::new())]),
            }
        }
        Command::Php {
            action: crate::cli::PhpAction::Ext { action },
        } => php_ext_to_request(action)?,
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
        Command::Domain { action } => domain_request(action)?,
        Command::Tunnel { action } => tunnel_request(action),
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
        Command::Root { name, path, auto } => {
            validate_name(name)?;
            Request::SetWebRoot {
                name: name.clone(),
                path: if *auto { None } else { path.clone() },
            }
        }
        Command::FrontController { name, state } => {
            validate_name(name)?;
            Request::SetFrontController {
                name: name.clone(),
                enabled: state.is_on(),
            }
        }
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
        Command::Link { .. } => {
            return Err(ClientError::Usage(
                "link is handled locally, not over IPC".to_owned(),
            ));
        }
    })
}

/// Map a `yerd service <action>` to its wire request. Service ids are passed
/// through verbatim - the daemon returns `NotFound` for an unknown id.
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

/// Map a `yerd domain <action>` to its wire request, validating the site name
/// and domain shape client-side. `List` is handled locally (it needs the TLD to
/// render default domains), so it never reaches here.
fn domain_request(action: &crate::cli::DomainAction) -> Result<Request, ClientError> {
    use crate::cli::DomainAction;
    Ok(match action {
        DomainAction::List { .. } => {
            return Err(ClientError::Usage(
                "domain list is handled locally, not over IPC".to_owned(),
            ));
        }
        DomainAction::Add { site, domain } => {
            validate_name(site)?;
            validate_domain(domain)?;
            Request::AddDomain {
                name: site.clone(),
                domain: domain.clone(),
            }
        }
        DomainAction::Remove { site, domain } => {
            validate_name(site)?;
            validate_domain(domain)?;
            Request::RemoveDomain {
                name: site.clone(),
                domain: domain.clone(),
            }
        }
        DomainAction::Primary { site, domain } => {
            validate_name(site)?;
            validate_domain(domain)?;
            Request::SetPrimaryDomain {
                name: site.clone(),
                domain: domain.clone(),
            }
        }
        DomainAction::Reset { site } => {
            validate_name(site)?;
            Request::ResetDomains { name: site.clone() }
        }
    })
}

/// Light client-side shape check for a domain FQDN, for a clean exit-2 error
/// before connecting. The daemon is authoritative (it strips and validates
/// against the configured TLD); this only catches obvious typos: ASCII,
/// `[a-z0-9.*-]`, at least two labels, non-empty labels, and `*` only as the
/// leftmost label.
fn validate_domain(domain: &str) -> Result<(), ClientError> {
    let bad = |msg: &str| ClientError::Usage(format!("invalid domain {domain:?}: {msg}"));
    if domain.is_empty() {
        return Err(bad("must not be empty"));
    }
    let lowered = domain.to_ascii_lowercase();
    let trimmed = lowered.strip_suffix('.').unwrap_or(&lowered);
    let labels: Vec<&str> = trimmed.split('.').collect();
    if labels.len() < 2 {
        return Err(bad("must be a full domain including the TLD"));
    }
    for (i, label) in labels.iter().enumerate() {
        if label.is_empty() {
            return Err(bad("contains an empty label"));
        }
        if *label == "*" {
            if i != 0 {
                return Err(bad("'*' is only allowed as the leftmost label"));
            }
            continue;
        }
        if !label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(bad("labels may only contain [a-z0-9-] (or a leading '*')"));
        }
    }
    Ok(())
}

/// Map a `yerd tunnel <action>` to its wire request. `Install` and `Login` are
/// streamed jobs the CLI intercepts before this point; mapping them here keeps
/// the match total.
fn tunnel_request(action: &TunnelAction) -> Request {
    match action {
        TunnelAction::Install => Request::InstallCloudflaredStreamed,
        TunnelAction::Share { site } => Request::StartQuickTunnel { site: site.clone() },
        TunnelAction::Stop { site } => Request::StopTunnel { site: site.clone() },
        TunnelAction::Status => Request::TunnelStatus,
        TunnelAction::Login => Request::CloudflaredLogin,
        TunnelAction::Create { name } => Request::CreateNamedTunnel { name: name.clone() },
        TunnelAction::Delete { name } => Request::DeleteNamedTunnel { name: name.clone() },
        TunnelAction::List => Request::ListNamedTunnels,
        TunnelAction::Route { tunnel, hostname } => Request::RouteTunnelDns {
            tunnel: tunnel.clone(),
            hostname: hostname.clone(),
        },
        TunnelAction::SetHost {
            site,
            hostname,
            clear,
        } => Request::SetSiteTunnel {
            site: site.clone(),
            hostname: if *clear { None } else { hostname.clone() },
        },
        TunnelAction::Publish => Request::StartNamedTunnel,
        TunnelAction::Unpublish => Request::StopNamedTunnel,
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

/// Map a `yerd php ext` action to its wire request, validating the version and
/// (for `add`) the name/path client-side so a bad argument fails before connect.
fn php_ext_to_request(action: &crate::cli::PhpExtAction) -> Result<Request, ClientError> {
    use crate::cli::PhpExtAction;
    Ok(match action {
        PhpExtAction::Add {
            version,
            path,
            zend,
            name,
        } => {
            let v = parse_php(version)?;
            let path_str = path
                .to_str()
                .ok_or_else(|| ClientError::Usage("extension path must be valid UTF-8".to_owned()))?
                .to_owned();
            let derived = name
                .clone()
                .or_else(|| yerd_core::php_extensions::default_name_from_path(&path_str))
                .unwrap_or_default();
            yerd_core::php_extensions::validate_entry(&derived, &path_str, *zend)
                .map_err(|e| ClientError::Usage(e.to_string()))?;
            Request::AddPhpExtension {
                version: v,
                path: path_str,
                name: name.clone(),
                zend: *zend,
            }
        }
        PhpExtAction::Remove { version, name } => Request::RemovePhpExtension {
            version: parse_php(version)?,
            name: name.clone(),
        },
        PhpExtAction::List => Request::ListPhpExtensions,
    })
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

/// A site's effective domain set + primary FQDN, derived from a [`SiteEntry`] and
/// the configured `tld`. For an effectively-default site the daemon omits the
/// domain fields, so the primary/domains are synthesized as `{name}.{tld}`.
#[must_use]
pub fn site_domains(entry: &SiteEntry, tld: &str) -> (String, Vec<String>) {
    let default = format!("{}.{tld}", entry.site.name());
    let primary = entry
        .primary_domain
        .clone()
        .unwrap_or_else(|| default.clone());
    let domains = if entry.domains.is_empty() {
        vec![default]
    } else {
        entry.domains.clone()
    };
    (primary, domains)
}

/// Render `yerd domain list [site]`. `tld` comes from a `DaemonInfo` round-trip
/// (needed to show default `{name}.{tld}` domains). With `filter`, shows only
/// that site (exit 1 if absent).
#[must_use]
pub fn render_domains(
    sites: &[SiteEntry],
    tld: &str,
    filter: Option<&str>,
    json: bool,
) -> Rendered {
    let selected: Vec<&SiteEntry> = match filter {
        Some(f) => {
            let f = f.to_ascii_lowercase();
            sites.iter().filter(|e| e.site.name() == f).collect()
        }
        None => sites.iter().collect(),
    };

    if let Some(f) = filter {
        if selected.is_empty() {
            return Rendered::err(format!("no site named {f:?}"));
        }
    }

    if json {
        let items: Vec<_> = selected
            .iter()
            .map(|e| {
                let (primary, domains) = site_domains(e, tld);
                serde_json::json!({
                    "name": e.site.name(),
                    "primary": primary,
                    "domains": domains,
                    "apex_shadowed_by": e.apex_shadowed_by,
                })
            })
            .collect();
        let body = serde_json::to_string(&serde_json::json!({ "domains": items }))
            .unwrap_or_else(|_| "{\"domains\":[]}".to_owned());
        return Rendered::ok(body);
    }

    if selected.is_empty() {
        return Rendered::ok("No sites yet.".to_owned());
    }
    let mut out = String::new();
    for e in selected {
        let (primary, domains) = site_domains(e, tld);
        let list = domains
            .iter()
            .map(|d| {
                if *d == primary {
                    format!("{d} (primary)")
                } else {
                    d.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(out, "{}: {list}", e.site.name());
        if let Some(by) = &e.apex_shadowed_by {
            let _ = write!(out, "  [apex shadowed by {by}]");
        }
        out.push('\n');
    }
    if out.ends_with('\n') {
        out.pop();
    }
    Rendered::ok(out)
}

/// Lowercase display name for a wire channel.
fn channel_str(c: Channel) -> &'static str {
    match c {
        Channel::Edge => "edge",
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
        UpdateSource::Cached => "cached (offline - last known values)",
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
/// document root is irrelevant - only the name is checked).
pub(crate) fn validate_name(name: &str) -> Result<(), ClientError> {
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
        Response::PhpExtensions { by_version } => Rendered::ok(format_php_extensions(by_version)),
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
        Response::Tunnels {
            tunnels,
            cloudflared,
        } => Rendered::ok(format_tunnels(tunnels, cloudflared)),
        Response::NamedTunnels {
            tunnels,
            sites,
            zone,
        } => Rendered::ok(format_named_tunnels(tunnels, sites, zone.as_deref())),
        Response::UpdateStatus {
            current,
            latest_stable,
            latest_edge,
            channel,
            available,
            target,
            ahead_of_stable,
            source,
            checked_at_epoch: _,
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

/// Renders the `yerd sites` table. The optional WORDPRESS and DOMAIN columns are
/// added only when at least one listed site needs them, so the common case's
/// table stays unchanged; full per-site domain lists live in `yerd domain list`.
fn format_sites(sites: &[SiteEntry]) -> String {
    if sites.is_empty() {
        return "no sites".to_owned();
    }
    let show_wordpress = sites.iter().any(|entry| entry.is_wordpress);
    let show_domain = sites
        .iter()
        .any(|e| e.primary_domain.is_some() || e.apex_shadowed_by.is_some());
    let mut out = String::from("NAME\tKIND\tPHP\tSECURE\tSERVED\tDOCROOT\tFRONT-CTRL");
    if show_domain {
        out.push_str("\tDOMAIN");
    }
    if show_wordpress {
        out.push_str("\tWORDPRESS");
    }
    for entry in sites {
        let s = &entry.site;
        let kind = match s.kind() {
            SiteKind::Parked => "parked",
            SiteKind::Linked => "linked",
        };
        let served = if s.web_subpath().as_os_str().is_empty() {
            "/".to_owned()
        } else {
            s.web_subpath().display().to_string()
        };
        let front = if entry.uses_front_controller {
            "index.php"
        } else {
            "direct"
        };
        let _ = write!(
            out,
            "\n{}\t{}\t{}\t{}\t{}\t{}\t{}",
            s.name(),
            kind,
            s.php(),
            s.secure(),
            served,
            s.document_root().display(),
            front
        );
        if show_domain {
            let domain = match (&entry.primary_domain, &entry.apex_shadowed_by) {
                (_, Some(by)) => format!("apex shadowed by {by}"),
                (Some(p), None) => p.clone(),
                (None, None) => "-".to_owned(),
            };
            let _ = write!(out, "\t{domain}");
        }
        if show_wordpress {
            let wp = if entry.is_wordpress { "yes" } else { "-" };
            let _ = write!(out, "\t{wp}");
        }
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
        _ => "unknown",
    }
}

/// Flatten tab/CR/LF in a value so a folded or multi-line mail header can't
/// break the tab-separated `yerd mail list` table (the `--json` path needs no
/// such treatment - serde already escapes control bytes).
fn flatten_cell(s: &str) -> String {
    s.replace(['\t', '\r', '\n'], " ")
}

/// Render `yerd mail list` - a tab-separated table of captured emails.
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

/// Render `yerd mail show <id>` - headers followed by the text body (falling
/// back to a note when only an HTML body is present).
fn format_mail(mail: &yerd_ipc::MailDetail) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "From:    {}", mail.from);
    let _ = writeln!(out, "To:      {}", mail.to.join(", "));
    let _ = writeln!(out, "Subject: {}", mail.subject);
    out.push('\n');
    match (&mail.text_body, &mail.html_body) {
        (Some(text), _) => out.push_str(text),
        (None, Some(_)) => out.push_str("(HTML-only message - open it in the GUI viewer)"),
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
        if s.installed_versions.is_empty() {
            let _ = write!(
                out,
                "\n{}\tnot installed\t-\t-\t{}\t-",
                s.service, s.enabled
            );
            continue;
        }
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

/// Render `yerd tools` as a tab-separated table (tool, status, commands, location).
fn format_tools(tools: &[ToolStatus]) -> String {
    if tools.is_empty() {
        return "no tools".to_owned();
    }
    let mut out = String::from("TOOL\tSTATUS\tCOMMANDS\tLOCATION");
    for t in tools {
        let status = if t.installed {
            t.version.as_deref().unwrap_or("installed")
        } else if t.external {
            "external"
        } else {
            "not installed"
        };
        let location = t.external_path.as_deref().unwrap_or("-");
        let _ = write!(
            out,
            "\n{}\t{}\t{}\t{}",
            t.id,
            status,
            t.binaries.join(","),
            location
        );
    }
    out
}

fn format_tunnels(tunnels: &[TunnelInfo], cloudflared: &CloudflaredStatus) -> String {
    let cf = if cloudflared.installed {
        cloudflared.version.as_deref().map_or_else(
            || "cloudflared: installed".to_owned(),
            |v| format!("cloudflared: {v}"),
        )
    } else {
        "cloudflared: not installed (run `yerd tunnel install`)".to_owned()
    };
    if tunnels.is_empty() {
        return format!("{cf}\nno active tunnels");
    }
    let mut out = format!("{cf}\n\nSITE\tSTATE\tURL");
    for t in tunnels {
        let state = match t.state {
            TunnelRunState::Running => "running",
            TunnelRunState::Failed => "failed",
            _ => "unknown",
        };
        let target = t.url.as_deref().or(t.hostname.as_deref()).unwrap_or("-");
        let _ = write!(out, "\n{}\t{}\t{}", t.site, state, target);
    }
    out
}

fn format_named_tunnels(
    tunnels: &[yerd_ipc::NamedTunnelMeta],
    sites: &[yerd_ipc::SiteHostname],
    zone: Option<&str>,
) -> String {
    let mut out = if tunnels.is_empty() {
        "no named tunnels".to_owned()
    } else {
        let mut s = String::from("NAME\tUUID");
        for t in tunnels {
            let _ = write!(s, "\n{}\t{}", t.name, t.uuid);
        }
        s
    };
    if let Some(zone) = zone {
        let _ = write!(out, "\n\nauthorized domain: {zone}");
    }
    if !sites.is_empty() {
        out.push_str("\n\nEXPOSED SITE\tHOSTNAME");
        for s in sites {
            let _ = write!(out, "\n{}\t{}", s.site, s.hostname);
        }
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
        format!("no PHP versions installed (default: {default}) - `yerd install php {default}`")
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
                    let _ = write!(line, " - update available: {} → {}", u.installed, u.latest);
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

/// Render the custom-extension registry, grouped by version, tagging any entry
/// whose `.so` is missing on disk.
fn format_php_extensions(
    by_version: &std::collections::BTreeMap<PhpVersion, Vec<yerd_ipc::PhpExtInfo>>,
) -> String {
    use std::fmt::Write as _;
    if by_version.is_empty() {
        return "no custom PHP extensions registered".to_owned();
    }
    let mut out = String::new();
    for (v, exts) in by_version {
        let _ = writeln!(out, "PHP {v}:");
        for e in exts {
            let kind = if e.zend {
                "zend_extension"
            } else {
                "extension"
            };
            let missing = if e.present { "" } else { "  (missing!)" };
            let _ = writeln!(out, "  {} [{kind}] {}{missing}", e.name, e.path);
        }
    }
    out.trim_end().to_owned()
}

/// Render a [`StatusReport`] as a human-readable block.
fn format_status(r: &StatusReport) -> String {
    use std::fmt::Write;
    let mut s = String::new();
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
    let version = if r.daemon_version.is_empty() {
        "unknown"
    } else {
        &r.daemon_version
    };
    let _ = writeln!(s, "version   {version}");
    let _ = writeln!(s, "tld       .{}", r.tld);
    let redirected = r.port_redirect == Some(true);
    if let Some(u) = r.web_unbound {
        let _ = writeln!(
            s,
            "http      not serving - couldn't bind {} (run `yerd doctor`)",
            u.http
        );
        let _ = writeln!(s, "https     not serving - couldn't bind {}", u.https);
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
        let _ = writeln!(
            s,
            "dns       not resolving - couldn't bind port {port} (run `yerd doctor`)"
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
    if r.resolver_installed == Some(false) && r.web_unbound.is_none() {
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
    #[allow(clippy::too_many_lines)]
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
            to_request(&Command::Unlink { name: "foo".into() }).unwrap(),
            Request::Unlink { name: "foo".into() }
        );
        assert_eq!(
            to_request(&Command::Unpark {
                path: PathBuf::from("/srv/sites")
            })
            .unwrap(),
            Request::Unpark {
                path: "/srv/sites".into()
            }
        );
        assert_eq!(
            to_request(&Command::List {
                target: crate::cli::ListTarget::Parked
            })
            .unwrap(),
            Request::ListParked
        );
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
        assert_eq!(
            to_request(&Command::FrontController {
                name: "foo".into(),
                state: crate::cli::OnOff::On,
            })
            .unwrap(),
            Request::SetFrontController {
                name: "foo".into(),
                enabled: true
            }
        );
        assert_eq!(
            to_request(&Command::FrontController {
                name: "foo".into(),
                state: crate::cli::OnOff::Off,
            })
            .unwrap(),
            Request::SetFrontController {
                name: "foo".into(),
                enabled: false
            }
        );
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
        match to_request(&Command::Use {
            first: "not-a-version".into(),
            version: None,
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
            checked_at_epoch: None,
        };
        let out = render(&resp, false).stdout;
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
        let resp = Response::UpdateStatus {
            current: "2.1.0-rc.3".into(),
            latest_stable: Some("2.0.5".into()),
            latest_edge: Some("2.1.0-rc.3".into()),
            channel: Channel::Stable,
            available: false,
            target: None,
            ahead_of_stable: true,
            source: UpdateSource::Cached,
            checked_at_epoch: None,
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

        let tools = render(
            &Response::Tools {
                tools: vec![
                    ToolStatus {
                        id: "node".into(),
                        display_name: "Node.js".into(),
                        installed: true,
                        version: Some("v24.17.0".into()),
                        binaries: vec!["node".into(), "npm".into(), "npx".into()],
                        external: false,
                        external_path: None,
                    },
                    ToolStatus {
                        id: "bun".into(),
                        display_name: "Bun".into(),
                        installed: false,
                        version: None,
                        binaries: vec!["bun".into(), "bunx".into()],
                        external: true,
                        external_path: Some("/opt/homebrew/bin/bun".into()),
                    },
                ],
            },
            false,
        );
        assert!(tools.stdout.contains("node"));
        assert!(tools.stdout.contains("v24.17.0"));
        assert!(tools.stdout.contains("npm"));
        assert!(tools.stdout.contains("external"));
        assert!(tools.stdout.contains("/opt/homebrew/bin/bun"));
        assert_eq!(tools.code, 0);

        let site = Site::linked("foo", "/srv/foo", PhpVersion::new(8, 3)).unwrap();
        let listed = render(
            &Response::Sites {
                sites: vec![SiteEntry {
                    site,
                    is_wordpress: false,
                    primary_domain: None,
                    domains: vec![],
                    apex_shadowed_by: None,
                    uses_front_controller: true,
                }],
            },
            false,
        );
        assert!(listed.stdout.contains("foo"));
        assert!(listed.stdout.contains("linked"));
        assert!(listed.stdout.contains("8.3"));
        assert!(
            !listed.stdout.contains("WORDPRESS"),
            "no WORDPRESS column when nothing listed is WordPress"
        );
        assert!(
            listed.stdout.contains("FRONT-CTRL"),
            "front-controller column header"
        );
        assert!(
            listed.stdout.contains("index.php"),
            "uses_front_controller=true renders as index.php"
        );
        assert_eq!(listed.code, 0);

        let blog = Site::parked("blog", "/srv/blog", PhpVersion::new(8, 3)).unwrap();
        let with_wp = render(
            &Response::Sites {
                sites: vec![SiteEntry {
                    site: blog,
                    is_wordpress: true,
                    primary_domain: None,
                    domains: vec![],
                    apex_shadowed_by: None,
                    uses_front_controller: false,
                }],
            },
            false,
        );
        assert!(with_wp.stdout.contains("WORDPRESS"));
        assert!(with_wp.stdout.contains("yes"));
        assert!(
            with_wp.stdout.contains("direct"),
            "uses_front_controller=false renders as direct"
        );

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
        assert!(r.stdout.contains("8.3 - update available: 8.3.6 → 8.3.31"));
        assert!(!r.stdout.contains("8.5 - update available"));
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
    fn php_ext_add_maps_and_defaults_name() {
        let req = to_request(&Command::Php {
            action: crate::cli::PhpAction::Ext {
                action: crate::cli::PhpExtAction::Add {
                    version: "8.5".into(),
                    path: "/opt/php/pecl/scrypt.so".into(),
                    zend: false,
                    name: None,
                },
            },
        })
        .unwrap();
        assert_eq!(
            req,
            Request::AddPhpExtension {
                version: PhpVersion::new(8, 5),
                path: "/opt/php/pecl/scrypt.so".to_string(),
                name: None,
                zend: false,
            }
        );
    }

    #[test]
    fn php_ext_add_rejects_non_absolute_path_client_side() {
        let err = to_request(&Command::Php {
            action: crate::cli::PhpAction::Ext {
                action: crate::cli::PhpExtAction::Add {
                    version: "8.5".into(),
                    path: "relative/scrypt.so".into(),
                    zend: false,
                    name: None,
                },
            },
        });
        assert!(matches!(err, Err(ClientError::Usage(_))));
    }

    #[test]
    fn php_ext_list_and_remove_map() {
        assert_eq!(
            to_request(&Command::Php {
                action: crate::cli::PhpAction::Ext {
                    action: crate::cli::PhpExtAction::List
                }
            })
            .unwrap(),
            Request::ListPhpExtensions
        );
        assert_eq!(
            to_request(&Command::Php {
                action: crate::cli::PhpAction::Ext {
                    action: crate::cli::PhpExtAction::Remove {
                        version: "8.5".into(),
                        name: "scrypt".into(),
                    }
                }
            })
            .unwrap(),
            Request::RemovePhpExtension {
                version: PhpVersion::new(8, 5),
                name: "scrypt".into(),
            }
        );
    }

    #[test]
    fn renders_php_extensions_grouped_with_missing_flag() {
        let r = render(
            &Response::PhpExtensions {
                by_version: std::collections::BTreeMap::from([(
                    PhpVersion::new(8, 5),
                    vec![
                        yerd_ipc::PhpExtInfo {
                            name: "scrypt".into(),
                            path: "/a/scrypt.so".into(),
                            zend: false,
                            present: true,
                        },
                        yerd_ipc::PhpExtInfo {
                            name: "xdebug".into(),
                            path: "/a/xdebug.so".into(),
                            zend: true,
                            present: false,
                        },
                    ],
                )]),
            },
            false,
        );
        assert_eq!(r.code, 0);
        assert!(r.stdout.contains("PHP 8.5:"));
        assert!(r.stdout.contains("scrypt [extension] /a/scrypt.so"));
        assert!(r
            .stdout
            .contains("xdebug [zend_extension] /a/xdebug.so  (missing!)"));
    }

    #[test]
    fn renders_empty_php_extensions() {
        let r = render(
            &Response::PhpExtensions {
                by_version: std::collections::BTreeMap::new(),
            },
            false,
        );
        assert!(r.stdout.contains("no custom PHP extensions registered"));
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
                php_trusts_ca: None,
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
            shared_sites: 0,
            symlink_protection: true,
            shadows: vec![],
        }
    }

    #[test]
    fn domain_add_remove_primary_reset_map_to_requests() {
        use crate::cli::DomainAction;
        assert_eq!(
            to_request(&Command::Domain {
                action: DomainAction::Add {
                    site: "foo".into(),
                    domain: "api.foo.test".into(),
                },
            })
            .unwrap(),
            Request::AddDomain {
                name: "foo".into(),
                domain: "api.foo.test".into(),
            }
        );
        assert_eq!(
            to_request(&Command::Domain {
                action: DomainAction::Primary {
                    site: "foo".into(),
                    domain: "corp.test".into(),
                },
            })
            .unwrap(),
            Request::SetPrimaryDomain {
                name: "foo".into(),
                domain: "corp.test".into(),
            }
        );
        assert_eq!(
            to_request(&Command::Domain {
                action: DomainAction::Reset { site: "foo".into() },
            })
            .unwrap(),
            Request::ResetDomains { name: "foo".into() }
        );
    }

    #[test]
    fn domain_list_is_handled_locally() {
        use crate::cli::DomainAction;
        assert!(matches!(
            to_request(&Command::Domain {
                action: DomainAction::List { site: None },
            }),
            Err(ClientError::Usage(_))
        ));
    }

    #[test]
    fn validate_domain_accepts_and_rejects() {
        assert!(validate_domain("api.foo.test").is_ok());
        assert!(validate_domain("*.foo.test").is_ok());
        assert!(validate_domain("foo").is_err()); // needs a TLD
        assert!(validate_domain("foo.*.test").is_err()); // misplaced wildcard
        assert!(validate_domain("a_b.test").is_err()); // bad char
        assert!(validate_domain("foo..test").is_err()); // empty label
    }

    #[test]
    fn render_domains_marks_primary_and_shadow() {
        let e = SiteEntry {
            site: Site::linked("blog", "/srv/blog", PhpVersion::new(8, 3)).unwrap(),
            is_wordpress: false,
            primary_domain: Some("corp.test".into()),
            domains: vec!["corp.test".into(), "*.blog.test".into()],
            apex_shadowed_by: Some("shop".into()),
            uses_front_controller: false,
        };
        let r = render_domains(&[e], "test", None, false);
        assert!(r.stdout.contains("corp.test (primary)"));
        assert!(r.stdout.contains("*.blog.test"));
        assert!(r.stdout.contains("apex shadowed by shop"));
    }

    #[test]
    fn render_domains_synthesizes_default_domain() {
        let e = SiteEntry {
            site: Site::linked("foo", "/srv/foo", PhpVersion::new(8, 3)).unwrap(),
            is_wordpress: false,
            primary_domain: None,
            domains: vec![],
            apex_shadowed_by: None,
            uses_front_controller: false,
        };
        let r = render_domains(&[e], "test", None, false);
        assert!(r.stdout.contains("foo.test (primary)"));
    }

    #[test]
    fn render_domains_unknown_site_filter_errors() {
        let r = render_domains(&[], "test", Some("ghost"), false);
        assert_eq!(r.code, 1);
        assert!(r.stderr.contains("ghost"));
    }

    #[test]
    fn fmt_port_distinguishes_fallback_from_redirect() {
        let fell_back = PortStatus {
            requested: 80,
            bound: 8080,
            fell_back: true,
        };
        assert_eq!(fmt_port(fell_back, false), "80 → 8080 (fallback)");
        assert_eq!(fmt_port(fell_back, true), "80 → 8080 (redirected)");
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
        assert!(out.contains("not serving - couldn't bind 8080"), "{out}");
        assert!(out.contains("not serving - couldn't bind 8443"), "{out}");
        assert!(!out.contains("→ 0"), "{out}");
    }

    #[test]
    fn status_degraded_dns_shows_not_resolving() {
        let mut r = sample_report();
        r.dns_unbound = Some(1053);
        let out = format_status(&r);
        assert!(
            out.contains("not resolving - couldn't bind port 1053"),
            "{out}"
        );
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
        assert!(out.stdout.contains("installed: unknown"));
        assert!(out.stdout.contains("1.52 0.48 0.05"));
        assert!(out.stdout.contains("8.5 (default)  running"));
        assert!(out.stdout.contains("pid 99"));
    }

    #[test]
    fn status_shows_unknown_for_empty_daemon_version() {
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
        assert_eq!(r.code, 0);
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

    #[test]
    fn maps_services_command() {
        assert_eq!(
            to_request(&Command::Services).unwrap(),
            Request::ListServices
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn maps_every_service_action() {
        use crate::cli::ServiceAction;
        assert_eq!(
            to_request(&Command::Service {
                action: ServiceAction::Available
            })
            .unwrap(),
            Request::AvailableServices
        );
        assert_eq!(
            to_request(&Command::Service {
                action: ServiceAction::Install {
                    service: "redis".into(),
                    version: "8".into()
                }
            })
            .unwrap(),
            Request::InstallService {
                service: "redis".into(),
                version: "8".into()
            }
        );
        assert_eq!(
            to_request(&Command::Service {
                action: ServiceAction::ChangeVersion {
                    service: "mysql".into(),
                    version: "9.1.0".into()
                }
            })
            .unwrap(),
            Request::ChangeServiceVersion {
                service: "mysql".into(),
                version: "9.1.0".into()
            }
        );
        assert_eq!(
            to_request(&Command::Service {
                action: ServiceAction::Uninstall {
                    service: "mariadb".into(),
                    version: "11".into(),
                    purge: true
                }
            })
            .unwrap(),
            Request::UninstallService {
                service: "mariadb".into(),
                version: "11".into(),
                purge: true
            }
        );
        assert_eq!(
            to_request(&Command::Service {
                action: ServiceAction::Start {
                    service: "redis".into()
                }
            })
            .unwrap(),
            Request::StartService {
                service: "redis".into()
            }
        );
        assert_eq!(
            to_request(&Command::Service {
                action: ServiceAction::Stop {
                    service: "redis".into()
                }
            })
            .unwrap(),
            Request::StopService {
                service: "redis".into()
            }
        );
        assert_eq!(
            to_request(&Command::Service {
                action: ServiceAction::Restart {
                    service: "redis".into()
                }
            })
            .unwrap(),
            Request::RestartService {
                service: "redis".into()
            }
        );
        assert_eq!(
            to_request(&Command::Service {
                action: ServiceAction::SetPort {
                    service: "redis".into(),
                    port: 6380
                }
            })
            .unwrap(),
            Request::SetServicePort {
                service: "redis".into(),
                port: 6380
            }
        );
        assert_eq!(
            to_request(&Command::Service {
                action: ServiceAction::Logs {
                    service: "mysql".into(),
                    lines: 50
                }
            })
            .unwrap(),
            Request::ServiceLogs {
                service: "mysql".into(),
                lines: 50
            }
        );
    }

    #[test]
    fn maps_every_db_action() {
        use crate::cli::DbAction;
        assert_eq!(
            to_request(&Command::Db {
                action: DbAction::List {
                    service: "mysql".into()
                }
            })
            .unwrap(),
            Request::ListDatabases {
                service: "mysql".into()
            }
        );
        assert_eq!(
            to_request(&Command::Db {
                action: DbAction::Create {
                    service: "mysql".into(),
                    name: "app".into()
                }
            })
            .unwrap(),
            Request::CreateDatabase {
                service: "mysql".into(),
                name: "app".into()
            }
        );
        assert_eq!(
            to_request(&Command::Db {
                action: DbAction::Drop {
                    service: "mysql".into(),
                    name: "app".into()
                }
            })
            .unwrap(),
            Request::DropDatabase {
                service: "mysql".into(),
                name: "app".into()
            }
        );
        assert_eq!(
            to_request(&Command::Db {
                action: DbAction::Backup {
                    service: "mysql".into(),
                    name: "app".into(),
                    path: PathBuf::from("dump.sql")
                }
            })
            .unwrap(),
            Request::BackupDatabase {
                service: "mysql".into(),
                name: "app".into(),
                path: PathBuf::from("dump.sql")
            }
        );
        assert_eq!(
            to_request(&Command::Db {
                action: DbAction::Restore {
                    service: "mysql".into(),
                    name: "app".into(),
                    path: PathBuf::from("dump.sql")
                }
            })
            .unwrap(),
            Request::RestoreDatabase {
                service: "mysql".into(),
                name: "app".into(),
                path: PathBuf::from("dump.sql")
            }
        );
    }

    #[test]
    fn maps_every_mail_action() {
        use crate::cli::MailAction;
        assert_eq!(
            to_request(&Command::Mail {
                action: MailAction::List
            })
            .unwrap(),
            Request::ListMails
        );
        assert_eq!(
            to_request(&Command::Mail {
                action: MailAction::Show { id: "abc".into() }
            })
            .unwrap(),
            Request::GetMail { id: "abc".into() }
        );
        assert_eq!(
            to_request(&Command::Mail {
                action: MailAction::Clear
            })
            .unwrap(),
            Request::ClearMails
        );
    }

    #[test]
    fn install_php_rejects_bad_version() {
        match to_request(&Command::Install {
            target: crate::cli::InstallTarget::Php {
                version: "not-a-version".into(),
            },
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
        match to_request(&Command::Restart {
            target: crate::cli::RestartTarget::Php {
                version: Some("xx".into()),
            },
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
        match to_request(&Command::Uninstall {
            target: Some(crate::cli::UninstallTarget::Php {
                version: "xx".into(),
            }),
            yes: false,
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
        match to_request(&Command::Update {
            target: Some(crate::cli::UpdateTarget::Php {
                version: Some("xx".into()),
            }),
            yes: false,
            edge: false,
            stable: false,
            force: false,
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
    }

    #[test]
    fn unset_unknown_php_setting_is_usage_error() {
        match to_request(&Command::Unset {
            target: crate::cli::UnsetTarget::Php {
                setting: "not_a_setting".into(),
            },
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
    }

    #[test]
    fn root_rejects_bad_name() {
        match to_request(&Command::Root {
            name: "bad name".into(),
            path: None,
            auto: true,
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
    }

    #[test]
    fn update_php_with_self_update_flags_each_error() {
        for (yes, edge, stable, force) in [
            (true, false, false, false),
            (false, true, false, false),
            (false, false, true, false),
            (false, false, false, true),
        ] {
            assert!(
                matches!(
                    to_request(&Command::Update {
                        target: Some(crate::cli::UpdateTarget::Php { version: None }),
                        yes,
                        edge,
                        stable,
                        force,
                    }),
                    Err(ClientError::Usage(_))
                ),
                "flags y={yes} e={edge} s={stable} f={force} should be a usage error"
            );
        }
    }

    #[test]
    fn bare_update_stable_flag_overrides_channel() {
        assert_eq!(
            to_request(&Command::Update {
                target: None,
                yes: false,
                edge: false,
                stable: true,
                force: false,
            })
            .unwrap(),
            Request::CheckUpdate {
                channel: Some(Channel::Stable)
            }
        );
    }

    #[test]
    fn local_only_commands_are_usage_errors() {
        for cmd in [
            Command::Uninstall {
                target: None,
                yes: false,
            },
            Command::Elevate { target: None },
            Command::Unelevate { target: None },
            Command::Elevate {
                target: Some(crate::cli::ElevateTarget::Trust),
            },
            Command::Unelevate {
                target: Some(crate::cli::ElevateTarget::Resolver),
            },
            Command::Path {
                action: crate::cli::PathAction::Install,
            },
            Command::Link {
                name_or_path: None,
                path: None,
            },
        ] {
            match to_request(&cmd) {
                Err(ClientError::Usage(_)) => {}
                other => panic!("expected Usage error for {cmd:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn channel_from_flags_table() {
        assert_eq!(channel_from_flags(true, false), Some(Channel::Edge));
        assert_eq!(channel_from_flags(false, true), Some(Channel::Stable));
        assert_eq!(channel_from_flags(false, false), None);
        assert_eq!(channel_from_flags(true, true), Some(Channel::Edge));
    }

    fn service_status(installed: Vec<String>) -> ServiceStatus {
        ServiceStatus {
            service: "redis".into(),
            display_name: "Redis".into(),
            installed_versions: installed,
            selected_version: None,
            state: ServiceRunState::Running,
            pid: Some(42),
            listen: Some("127.0.0.1:6379".into()),
            port: 6379,
            enabled: true,
            supports_databases: false,
        }
    }

    #[test]
    fn renders_services_table_and_states() {
        let empty = render(&Response::Services { services: vec![] }, false);
        assert_eq!(empty.stdout, "no services");
        assert_eq!(empty.code, 0);

        let installed = render(
            &Response::Services {
                services: vec![service_status(vec!["7".into(), "8".into()])],
            },
            false,
        );
        assert!(installed.stdout.contains("SERVICE\tSTATE\tPORT"));
        assert!(installed.stdout.contains("redis\trunning\t6379"));
        assert!(installed.stdout.contains("\t8\t"), "{}", installed.stdout);
        assert!(installed.stdout.contains("7,8"));

        let not_installed = render(
            &Response::Services {
                services: vec![service_status(vec![])],
            },
            false,
        );
        assert!(not_installed.stdout.contains("redis\tnot installed\t-\t-"));

        let mut stopped = service_status(vec!["8".into()]);
        stopped.state = ServiceRunState::Stopped;
        stopped.selected_version = Some("8".into());
        assert!(render(
            &Response::Services {
                services: vec![stopped]
            },
            false
        )
        .stdout
        .contains("redis\tstopped"));

        let mut failed = service_status(vec!["8".into()]);
        failed.state = ServiceRunState::Failed;
        assert!(render(
            &Response::Services {
                services: vec![failed]
            },
            false
        )
        .stdout
        .contains("redis\tfailed"));
    }

    #[test]
    fn renders_available_services() {
        let empty = render(&Response::AvailableServices { services: vec![] }, false);
        assert_eq!(empty.stdout, "no services available");

        let listed = render(
            &Response::AvailableServices {
                services: vec![
                    ServiceAvailability {
                        service: "redis".into(),
                        available: vec!["7".into(), "8".into()],
                        installed: vec!["8".into()],
                    },
                    ServiceAvailability {
                        service: "mysql".into(),
                        available: vec![],
                        installed: vec![],
                    },
                ],
            },
            false,
        );
        assert!(listed.stdout.contains("SERVICE\tAVAILABLE\tINSTALLED"));
        assert!(listed.stdout.contains("redis\t7,8\t8"));
        assert!(listed.stdout.contains("mysql\t-\t-"));
    }

    #[test]
    fn renders_service_logs() {
        let empty = render(&Response::ServiceLogs { lines: vec![] }, false);
        assert_eq!(empty.stdout, "no log output");

        let lines = render(
            &Response::ServiceLogs {
                lines: vec!["line one".into(), "line two".into()],
            },
            false,
        );
        assert_eq!(lines.stdout, "line one\nline two");
        assert_eq!(lines.code, 0);
    }

    #[test]
    fn renders_databases() {
        let empty = render(&Response::Databases { databases: vec![] }, false);
        assert_eq!(empty.stdout, "no databases");

        let listed = render(
            &Response::Databases {
                databases: vec![
                    yerd_ipc::DatabaseSummary { name: "app".into() },
                    yerd_ipc::DatabaseSummary {
                        name: "blog".into(),
                    },
                ],
            },
            false,
        );
        assert_eq!(listed.stdout, "app\nblog");
    }

    #[test]
    fn renders_mail_list() {
        let empty = render(&Response::Mails { mails: vec![] }, false);
        assert_eq!(empty.stdout, "no captured emails");

        let listed = render(
            &Response::Mails {
                mails: vec![
                    yerd_ipc::MailSummary {
                        id: "id1".into(),
                        from: "a@example.com".into(),
                        to: vec!["b@example.com".into()],
                        subject: "hello\tthere\nworld".into(),
                        date_epoch: 0,
                        read: false,
                    },
                    yerd_ipc::MailSummary {
                        id: "id2".into(),
                        from: "c@example.com".into(),
                        to: vec![],
                        subject: String::new(),
                        date_epoch: 0,
                        read: false,
                    },
                ],
            },
            false,
        );
        assert!(listed.stdout.contains("ID\tFROM\tSUBJECT"));
        assert!(listed
            .stdout
            .contains("id1\ta@example.com\thello there world"));
        assert!(listed.stdout.contains("id2\tc@example.com\t(no subject)"));
    }

    #[test]
    fn renders_mail_detail_body_variants() {
        let base = yerd_ipc::MailDetail {
            id: "id1".into(),
            from: "a@example.com".into(),
            to: vec!["b@example.com".into(), "c@example.com".into()],
            subject: "Hi".into(),
            date_epoch: 0,
            headers: vec![],
            html_body: None,
            text_body: Some("plain body".into()),
        };
        let text = render(
            &Response::Mail {
                mail: Box::new(base.clone()),
            },
            false,
        );
        assert!(text.stdout.contains("From:    a@example.com"));
        assert!(text
            .stdout
            .contains("To:      b@example.com, c@example.com"));
        assert!(text.stdout.contains("Subject: Hi"));
        assert!(text.stdout.contains("plain body"));

        let mut html_only = base.clone();
        html_only.text_body = None;
        html_only.html_body = Some("<p>hi</p>".into());
        assert!(render(
            &Response::Mail {
                mail: Box::new(html_only)
            },
            false
        )
        .stdout
        .contains("HTML-only message"));

        let mut empty = base;
        empty.text_body = None;
        empty.html_body = None;
        assert!(render(
            &Response::Mail {
                mail: Box::new(empty)
            },
            false
        )
        .stdout
        .contains("(empty message)"));
    }
}
