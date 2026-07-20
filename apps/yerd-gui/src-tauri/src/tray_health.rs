//! Overall tray health (green / amber / red) derived from a [`StatusReport`].
//!
//! `derive_health` is mirrored in `apps/yerd-gui/src/lib/trayHealth.ts`.
//!
//! [`tray_dropdown_service_rows`] builds the tray panel / menu-bar Services list
//! (running/failed PHP pools + installed managed services). [`service_rows`] is
//! the fuller diagnostic list (Proxy + every pool + every managed instance).

use yerd_ipc::{PoolRunState, ServiceRunState, StatusReport};

/// Aggregate tray health for the menu-bar icon and status header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrayHealth {
    /// Daemon unreachable or critically degraded.
    #[default]
    Bad,
    /// Daemon up but with degraded privileges / idle enabled services.
    Warn,
    /// Everything looks healthy.
    Ok,
}

impl TrayHealth {
    /// RGB colour for the menu-bar status dot (bottom-right of the tray icon).
    pub const fn rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Ok => (52, 199, 89),    // green
            Self::Warn => (255, 159, 10), // amber
            Self::Bad => (235, 64, 52),   // red
        }
    }

    /// Glyph prefix for native menu service / status rows.
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Ok => "●",
            Self::Warn => "◐",
            Self::Bad => "○",
        }
    }
}

const PRIVILEGED_PORT_CEILING: u16 = 1024;

/// Derive tray health from a live status report. Callers that cannot reach the
/// daemon should use [`TrayHealth::Bad`] directly rather than inventing a report.
pub fn derive_health(report: &StatusReport) -> TrayHealth {
    if report.web_unbound.is_some()
        || report.dns_unbound.is_some()
        || report.foreign_web_listener == Some(true)
        || report.php.iter().any(|p| p.state == PoolRunState::Failed)
        || report
            .services
            .iter()
            .any(|s| s.state == ServiceRunState::Failed)
    {
        return TrayHealth::Bad;
    }

    let ports_degraded = ports_fell_privileged(report) && report.port_redirect != Some(true);
    let ca_bad = report.ca.trusted_system == Some(false);
    let resolver_bad = report.resolver_installed == Some(false);
    let enabled_stopped = report
        .services
        .iter()
        .any(|s| s.enabled && s.state == ServiceRunState::Stopped);

    if ports_degraded || ca_bad || resolver_bad || enabled_stopped {
        return TrayHealth::Warn;
    }

    TrayHealth::Ok
}

fn ports_fell_privileged(report: &StatusReport) -> bool {
    (report.http.requested < PRIVILEGED_PORT_CEILING && report.http.fell_back)
        || (report.https.requested < PRIVILEGED_PORT_CEILING && report.https.fell_back)
}

/// Per-row health for the synthetic Proxy / PHP lines and real services.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceRow {
    pub id: String,
    pub label: String,
    pub health: TrayHealth,
}

/// Build the full diagnostic service list: Proxy + every PHP pool + every managed
/// instance (including stopped). The tray UI uses [`tray_dropdown_service_rows`].
#[allow(dead_code)]
pub fn service_rows(report: &StatusReport) -> Vec<ServiceRow> {
    let mut rows = Vec::with_capacity(1 + report.php.len() + report.services.len());

    let proxy_health = if report.web_unbound.is_some() || report.foreign_web_listener == Some(true)
    {
        TrayHealth::Bad
    } else if ports_fell_privileged(report) && report.port_redirect != Some(true) {
        TrayHealth::Warn
    } else {
        TrayHealth::Ok
    };
    rows.push(ServiceRow {
        id: "proxy".into(),
        label: "Proxy".into(),
        health: proxy_health,
    });

    for pool in &report.php {
        let health = match pool.state {
            PoolRunState::Failed => TrayHealth::Bad,
            _ => TrayHealth::Ok,
        };
        rows.push(ServiceRow {
            id: format!("php:{}", pool.version),
            label: format!("PHP {}", pool.version),
            health,
        });
    }

    for s in &report.services {
        rows.push(managed_service_row(s));
    }

    rows
}

/// Running/failed PHP pools plus installed managed services — tray panel and the
/// menu-bar **Services** block (mirrors `trayHealth.ts::trayServiceRows`).
pub fn tray_dropdown_service_rows(report: &StatusReport) -> Vec<ServiceRow> {
    let php_count = report
        .php
        .iter()
        .filter(|p| matches!(p.state, PoolRunState::Running | PoolRunState::Failed))
        .count();
    let managed_count = report
        .services
        .iter()
        .filter(|s| is_installed_service(s))
        .count();
    let mut rows = Vec::with_capacity(php_count + managed_count);

    for pool in &report.php {
        if !matches!(pool.state, PoolRunState::Running | PoolRunState::Failed) {
            continue;
        }
        let health = match pool.state {
            PoolRunState::Failed => TrayHealth::Bad,
            _ => TrayHealth::Ok,
        };
        rows.push(ServiceRow {
            id: format!("php:{}", pool.version),
            label: format!("PHP {}", pool.version),
            health,
        });
    }

    for s in &report.services {
        if !is_installed_service(s) {
            continue;
        }
        rows.push(managed_service_row(s));
    }

    rows
}

fn is_installed_service(s: &yerd_ipc::ServiceStatus) -> bool {
    !s.installed_versions.is_empty() || s.site.is_some()
}

fn managed_service_row(s: &yerd_ipc::ServiceStatus) -> ServiceRow {
    let health = match s.state {
        ServiceRunState::Running => TrayHealth::Ok,
        ServiceRunState::Failed => TrayHealth::Bad,
        ServiceRunState::Stopped if s.enabled => TrayHealth::Warn,
        ServiceRunState::Stopped => TrayHealth::Ok,
        _ => TrayHealth::Warn,
    };
    let label = if let Some(site) = &s.site {
        format!("{} ({site})", s.display_name)
    } else {
        s.display_name.clone()
    };
    ServiceRow {
        id: s.service.clone(),
        label,
        health,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use yerd_core::PhpVersion;
    use yerd_ipc::{CaStatus, PhpPoolStatus, PortStatus, ServiceStatus, SiteCounts};

    fn base_report() -> StatusReport {
        StatusReport {
            daemon_pid: 1,
            uptime_secs: 10,
            daemon_rss_bytes: None,
            tld: "test".into(),
            http: PortStatus {
                requested: 80,
                bound: 80,
                fell_back: false,
            },
            https: PortStatus {
                requested: 443,
                bound: 443,
                fell_back: false,
            },
            dns_addr: "127.0.0.1:5353".parse::<SocketAddr>().unwrap(),
            ca: CaStatus {
                path: PathBuf::from("/tmp/ca.pem"),
                fingerprint: "a".repeat(64),
                trusted_system: Some(true),
                php_trusts_ca: Some(true),
                browser_trust: None,
            },
            resolver_installed: Some(true),
            port_redirect: None,
            foreign_web_listener: Some(false),
            resolver_backup: None,
            default_php: PhpVersion::new(8, 4),
            php: vec![PhpPoolStatus {
                version: PhpVersion::new(8, 4),
                installed_patch: Some("8.4.1".into()),
                state: PoolRunState::Running,
                pid: Some(2),
                listen: Some("/tmp/php.sock".into()),
                rss_bytes: None,
                update_available: None,
            }],
            sites: SiteCounts {
                parked: 0,
                linked: 1,
                secured: 1,
            },
            load_avg: None,
            daemon_version: "2.0.3".into(),
            services: vec![],
            mail: None,
            web_unbound: None,
            dns_unbound: None,
            boot_id: Some(1),
            shared_sites: 0,
            symlink_protection: true,
            shadows: vec![],
            mcp_enabled: false,
            lan_enabled: false,
            lan_ip: None,
            lan_setup_bound: None,
        }
    }

    #[test]
    fn healthy_report_is_ok() {
        assert_eq!(derive_health(&base_report()), TrayHealth::Ok);
    }

    #[test]
    fn web_unbound_is_bad() {
        let mut r = base_report();
        r.web_unbound = Some(yerd_ipc::UnboundWeb {
            http: 8080,
            https: 8443,
        });
        assert_eq!(derive_health(&r), TrayHealth::Bad);
    }

    #[test]
    fn failed_php_pool_is_bad() {
        let mut r = base_report();
        r.php[0].state = PoolRunState::Failed;
        assert_eq!(derive_health(&r), TrayHealth::Bad);
    }

    #[test]
    fn untrusted_ca_is_warn() {
        let mut r = base_report();
        r.ca.trusted_system = Some(false);
        assert_eq!(derive_health(&r), TrayHealth::Warn);
    }

    #[test]
    fn enabled_stopped_service_is_warn() {
        let mut r = base_report();
        r.services.push(ServiceStatus {
            service: "redis".into(),
            display_name: "Redis".into(),
            installed_versions: vec!["7".into()],
            selected_version: Some("7".into()),
            state: ServiceRunState::Stopped,
            pid: None,
            listen: None,
            port: 6379,
            enabled: true,
            supports_databases: false,
            type_id: "redis".into(),
            site: None,
            error: None,
        });
        assert_eq!(derive_health(&r), TrayHealth::Warn);
    }

    #[test]
    fn service_rows_include_proxy_and_per_pool_php() {
        let rows = service_rows(&base_report());
        assert_eq!(rows[0].id, "proxy");
        assert_eq!(rows[1].id, "php:8.4");
        assert_eq!(rows[1].label, "PHP 8.4");
        assert_eq!(rows[0].health, TrayHealth::Ok);
    }

    #[test]
    fn tray_dropdown_service_rows_include_php_and_installed_managed() {
        let mut r = base_report();
        r.services.push(ServiceStatus {
            service: "redis".into(),
            display_name: "Redis".into(),
            installed_versions: vec!["7".into()],
            selected_version: Some("7".into()),
            state: ServiceRunState::Running,
            pid: Some(9),
            listen: None,
            port: 6379,
            enabled: true,
            supports_databases: false,
            type_id: "redis".into(),
            site: None,
            error: None,
        });
        r.services.push(ServiceStatus {
            service: "postgres".into(),
            display_name: "PostgreSQL".into(),
            installed_versions: vec!["17".into()],
            selected_version: Some("17".into()),
            state: ServiceRunState::Running,
            pid: Some(10),
            listen: None,
            port: 5432,
            enabled: true,
            supports_databases: true,
            type_id: "postgres".into(),
            site: None,
            error: None,
        });
        let rows = tray_dropdown_service_rows(&r);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, "php:8.4");
        assert_eq!(rows[1].id, "redis");
        assert_eq!(rows[2].id, "postgres");
        assert_eq!(rows[2].label, "PostgreSQL");
    }

    #[test]
    fn tray_dropdown_service_rows_skip_uninstalled_engines() {
        let mut r = base_report();
        r.services.push(ServiceStatus {
            service: "postgres".into(),
            display_name: "PostgreSQL".into(),
            installed_versions: vec![],
            selected_version: None,
            state: ServiceRunState::Stopped,
            pid: None,
            listen: None,
            port: 5432,
            enabled: false,
            supports_databases: true,
            type_id: "postgres".into(),
            site: None,
            error: None,
        });
        let rows = tray_dropdown_service_rows(&r);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "php:8.4");
    }

    #[test]
    fn privileged_fallback_without_redirect_is_warn() {
        let mut r = base_report();
        r.http.fell_back = true;
        r.http.bound = 8080;
        r.port_redirect = Some(false);
        assert_eq!(derive_health(&r), TrayHealth::Warn);
    }
}
