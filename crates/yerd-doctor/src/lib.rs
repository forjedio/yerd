//! Pure diagnosis and fix-planning for `yerd doctor`.
//!
//! This crate is runtime-free and does no I/O: [`diagnose`] turns a
//! [`StatusReport`] into a list of [`Diagnosis`] findings, and
//! [`plan_auto_fixes`] returns the safe, unprivileged [`FixAction`]s the daemon
//! may apply automatically. The daemon performs the actual I/O (status assembly,
//! restarting pools) and re-runs [`diagnose`] afterwards to compute what still
//! needs manual attention.
//!
//! ## Why `plan_auto_fixes(&StatusReport)` and not `auto_fix(&Diagnosis)`
//!
//! A wire [`Diagnosis`] carries only strings, so it cannot hand back the typed
//! [`yerd_core::PhpVersion`] a [`FixAction::RestartFpm`] needs. Planning fixes
//! from the typed report instead keeps the action list precise.

#![forbid(unsafe_code)]

use yerd_core::PhpVersion;
use yerd_ipc::{Diagnosis, DiagnosisCode, PoolRunState, ServiceRunState, Severity, StatusReport};

/// Ports below this are privileged (need elevation to bind).
const PRIVILEGED_PORT_CEILING: u16 = 1024;

/// A safe, fast, unprivileged fix the daemon may apply automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FixAction {
    /// Restart the FPM pool for this PHP version.
    RestartFpm(PhpVersion),
}

/// Run every check against `report` and return the findings.
///
/// Findings are emitted in a stable order. When no `Warn`/`Fail` finding is
/// produced, a single [`DiagnosisCode::AllGood`] `Ok` finding is appended so the
/// caller always has something to show. `Option<bool>` probes that are `None`
/// ("couldn't determine") emit **no** finding — never a false-alarm warning.
#[must_use]
pub fn diagnose(report: &StatusReport) -> Vec<Diagnosis> {
    let mut out = Vec::new();

    // --- Port findings: a foreign listener squatting 80/443, or a privileged
    // fallback that needs elevation (see `port_findings`).
    out.extend(port_findings(report));

    // --- CA trust + resolver (each skipped when undeterminable).
    out.extend(trust_findings(report));

    // --- PHP install state (NoPhpInstalled suppresses DefaultPhpNotInstalled),
    // then failed FPM pools (one per pool; auto-fixable by restart).
    out.extend(php_state_findings(report));

    // --- Failed services (one finding per engine).
    out.extend(service_findings(report));

    // --- Available PHP updates (informational).
    out.extend(php_update_findings(report));

    // --- Replaced resolver file backed up (informational). The daemon only
    // sets this for a recent backup, so it auto-clears rather than nagging.
    out.extend(resolver_backup_finding(report));

    // --- No sites (informational).
    out.extend(no_sites_finding(report));

    // --- All-good summary when nothing is wrong.
    if !out
        .iter()
        .any(|d| matches!(d.severity, Severity::Warn | Severity::Fail))
    {
        out.push(Diagnosis {
            code: DiagnosisCode::AllGood,
            severity: Severity::Ok,
            title: "All checks passed".to_owned(),
            detail: "Daemon, ports, DNS, CA, and PHP look healthy.".to_owned(),
            remedy: None,
        });
    }

    out
}

/// CA-trust and resolver findings (each skipped when its probe is `None`).
fn trust_findings(report: &StatusReport) -> Vec<Diagnosis> {
    let mut out = Vec::new();
    if report.ca.trusted_system == Some(false) {
        out.push(warn(
            DiagnosisCode::CaNotTrusted,
            "Local CA not trusted",
            "HTTPS sites will show certificate warnings until the CA is trusted.".to_owned(),
            "sudo yerd elevate trust",
        ));
    }
    if report.resolver_installed == Some(false) {
        out.push(warn(
            DiagnosisCode::ResolverNotInstalled,
            "Resolver not installed",
            format!(
                "*.{} is not routed to Yerd's DNS responder ({}).",
                report.tld, report.dns_addr
            ),
            "sudo yerd elevate resolver",
        ));
    }
    out
}

/// PHP install-state findings: a missing install (which suppresses the
/// default-not-installed finding), then one finding per failed FPM pool.
fn php_state_findings(report: &StatusReport) -> Vec<Diagnosis> {
    let mut out = Vec::new();
    if report.php.is_empty() {
        out.push(fail(
            DiagnosisCode::NoPhpInstalled,
            "No PHP versions installed",
            "No site can be served until a PHP version is installed.".to_owned(),
            Some(format!("yerd install php {}", report.default_php)),
        ));
    } else if !report.php.iter().any(|p| p.version == report.default_php) {
        out.push(fail(
            DiagnosisCode::DefaultPhpNotInstalled,
            "Default PHP not installed",
            format!(
                "The configured default PHP {} is not installed.",
                report.default_php
            ),
            Some(format!("yerd install php {}", report.default_php)),
        ));
    }
    for pool in &report.php {
        if pool.state == PoolRunState::Failed {
            out.push(fail(
                DiagnosisCode::FpmPoolFailed,
                "PHP-FPM pool failed",
                format!("The PHP {} FPM pool is not running.", pool.version),
                Some(format!(
                    "fixed automatically by `yerd doctor fix`, or restart with `yerd use {}`",
                    pool.version
                )),
            ));
        }
    }
    out
}

/// One finding per failed database/cache service.
fn service_findings(report: &StatusReport) -> Vec<Diagnosis> {
    let mut out = Vec::new();
    for svc in &report.services {
        if svc.state == ServiceRunState::Failed {
            out.push(fail(
                DiagnosisCode::ServiceFailed,
                "Service failed",
                format!("The {} service is not running.", svc.display_name),
                Some(format!(
                    "restart with `yerd service restart {}`",
                    svc.service
                )),
            ));
        }
    }
    out
}

/// One informational finding per PHP version with a newer patch available.
fn php_update_findings(report: &StatusReport) -> Vec<Diagnosis> {
    let mut out = Vec::new();
    for pool in &report.php {
        if let Some(latest) = &pool.update_available {
            out.push(Diagnosis {
                code: DiagnosisCode::PhpUpdateAvailable,
                severity: Severity::Ok,
                title: "PHP update available".to_owned(),
                detail: format!("PHP {} can be updated to {latest}.", pool.version),
                remedy: Some(format!("yerd update php {}", pool.version)),
            });
        }
    }
    out
}

/// Informational finding when no sites are configured.
fn no_sites_finding(report: &StatusReport) -> Option<Diagnosis> {
    (report.sites.parked == 0 && report.sites.linked == 0).then(|| Diagnosis {
        code: DiagnosisCode::NoSites,
        severity: Severity::Ok,
        title: "No sites configured".to_owned(),
        detail: "Nothing is being served yet.".to_owned(),
        remedy: Some("yerd park <dir>  (or  yerd link <name> <dir>)".to_owned()),
    })
}

/// Informational finding when the daemon reports a recent backup of a replaced
/// `/etc/resolver/<tld>`. `Ok` severity with **no remedy** — the GUI renders
/// `remedy` as a copy-a-command chip, which would misrepresent this path/guidance
/// as a runnable command.
fn resolver_backup_finding(report: &StatusReport) -> Option<Diagnosis> {
    let path = report.resolver_backup.as_ref()?;
    Some(Diagnosis {
        code: DiagnosisCode::ResolverBackupSaved,
        severity: Severity::Ok,
        title: "Resolver file replaced".to_owned(),
        detail: format!(
            "Installing the .{} resolver replaced an existing /etc/resolver file; \
             your previous one was saved to {path}. Unelevating the resolver \
             restores it automatically, or you can delete the backup.",
            report.tld
        ),
        remedy: None,
    })
}

/// Return the safe, unprivileged fixes the daemon may apply for `report`.
///
/// Conservative by design: only failed FPM pools (restartable without
/// privilege) are auto-fixable. Privileged or slow remediation (CA trust,
/// resolver, setcap, PHP install) is left for the user to run.
#[must_use]
pub fn plan_auto_fixes(report: &StatusReport) -> Vec<FixAction> {
    report
        .php
        .iter()
        .filter(|p| p.state == PoolRunState::Failed)
        .map(|p| FixAction::RestartFpm(p.version))
        .collect()
}

/// Whether a finding with this `code` is one the daemon auto-fixes — used by the
/// daemon to drop already-handled findings from the "manual" remainder.
#[must_use]
pub fn is_auto_fixable(code: DiagnosisCode) -> bool {
    matches!(code, DiagnosisCode::FpmPoolFailed)
}

/// Findings about the privileged web ports (80/443), in stable order.
///
/// A non-Yerd process holding the port is the *cause* a plain fallback would
/// misattribute to "needs elevation", so when it's detected we surface that
/// instead and suppress the fallback advice (elevation can't bind a port
/// another process owns). On macOS the daemon still binds the rootless ports
/// even once elevated, so an active pf redirect (`port_redirect == Some(true)`)
/// means 80/443 are in fact reachable — also suppressing the fallback warning.
fn port_findings(report: &StatusReport) -> Vec<Diagnosis> {
    let mut out = Vec::new();
    let foreign_listener = report.foreign_web_listener == Some(true);
    if foreign_listener {
        out.push(warn(
            DiagnosisCode::ForeignWebListener,
            "Another process is using port 80/443",
            "A program other than Yerd is listening on a privileged web port (80/443). \
             Yerd can't serve your .test sites there until it's stopped."
                .to_owned(),
            "Stop the other web server (e.g. Apache, nginx, Valet), then `sudo yerd elevate ports`",
        ));
    }
    if privileged_fallback(report) && report.port_redirect != Some(true) && !foreign_listener {
        out.push(warn(
            DiagnosisCode::PortFallback,
            "Privileged ports not bound",
            format!(
                "HTTP {}→{}, HTTPS {}→{}: 80/443 need elevation, serving on the rootless ports.",
                report.http.requested,
                report.http.bound,
                report.https.requested,
                report.https.bound
            ),
            "sudo yerd elevate ports",
        ));
    }
    out
}

fn privileged_fallback(report: &StatusReport) -> bool {
    (report.http.requested < PRIVILEGED_PORT_CEILING && report.http.fell_back)
        || (report.https.requested < PRIVILEGED_PORT_CEILING && report.https.fell_back)
}

fn warn(code: DiagnosisCode, title: &str, detail: String, remedy: &str) -> Diagnosis {
    Diagnosis {
        code,
        severity: Severity::Warn,
        title: title.to_owned(),
        detail,
        remedy: Some(remedy.to_owned()),
    }
}

fn fail(code: DiagnosisCode, title: &str, detail: String, remedy: Option<String>) -> Diagnosis {
    Diagnosis {
        code,
        severity: Severity::Fail,
        title: title.to_owned(),
        detail,
        remedy,
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
    use yerd_ipc::{CaStatus, PhpPoolStatus, PortStatus, SiteCounts, StatusReport};

    /// A fully-healthy baseline report: privileged ports bound, CA trusted,
    /// resolver installed, default PHP running, one site.
    fn healthy() -> StatusReport {
        StatusReport {
            daemon_pid: 1,
            uptime_secs: 10,
            daemon_rss_bytes: Some(2048),
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
            dns_addr: "127.0.0.1:1053".parse().unwrap(),
            ca: CaStatus {
                path: "/x/ca.cert.pem".into(),
                fingerprint: "ab".repeat(32),
                trusted_system: Some(true),
            },
            resolver_installed: Some(true),
            port_redirect: None,
            foreign_web_listener: None,
            resolver_backup: None,
            default_php: PhpVersion::new(8, 5),
            php: vec![PhpPoolStatus {
                version: PhpVersion::new(8, 5),
                installed_patch: Some("8.5.6".into()),
                state: PoolRunState::Running,
                pid: Some(99),
                listen: Some("/run/fpm.sock".into()),
                rss_bytes: Some(1024),
                update_available: None,
            }],
            sites: SiteCounts {
                parked: 1,
                linked: 0,
                secured: 0,
            },
            load_avg: Some([10, 5, 1]),
            daemon_version: "2.0.1".into(),
            services: vec![],
            mail: None,
        }
    }

    fn codes(ds: &[Diagnosis]) -> Vec<DiagnosisCode> {
        ds.iter().map(|d| d.code).collect()
    }

    #[test]
    fn healthy_report_is_all_good_only() {
        let ds = diagnose(&healthy());
        assert_eq!(codes(&ds), vec![DiagnosisCode::AllGood]);
        assert!(plan_auto_fixes(&healthy()).is_empty());
    }

    #[test]
    fn resolver_backup_surfaces_as_ok_finding_with_no_remedy() {
        let mut r = healthy();
        assert!(!codes(&diagnose(&r)).contains(&DiagnosisCode::ResolverBackupSaved));

        r.resolver_backup =
            Some("/Library/Application Support/io.yerd.Yerd/resolver-backups/test-1.conf".into());
        let ds = diagnose(&r);
        let finding = ds
            .iter()
            .find(|d| d.code == DiagnosisCode::ResolverBackupSaved)
            .expect("backup finding present");
        assert_eq!(finding.severity, Severity::Ok);
        assert!(
            finding.remedy.is_none(),
            "info finding must not render a command chip"
        );
        assert!(finding.detail.contains("resolver-backups/test-1.conf"));
        // Informational → does not suppress AllGood and is not auto-fixable.
        assert!(codes(&ds).contains(&DiagnosisCode::AllGood));
        assert!(!is_auto_fixable(DiagnosisCode::ResolverBackupSaved));
    }

    #[test]
    fn privileged_fallback_warns_but_high_ports_do_not() {
        let mut r = healthy();
        r.http.requested = 80;
        r.http.bound = 8080;
        r.http.fell_back = true;
        assert!(codes(&diagnose(&r)).contains(&DiagnosisCode::PortFallback));

        // Configured unprivileged port that "fell back" is NOT a warning.
        let mut r2 = healthy();
        r2.http.requested = 8080;
        r2.http.bound = 8081;
        r2.http.fell_back = true;
        assert!(!codes(&diagnose(&r2)).contains(&DiagnosisCode::PortFallback));
    }

    #[test]
    fn active_port_redirect_suppresses_fallback_warning() {
        // Privileged port fell back, but a pf redirect is live (macOS): the
        // ports are reachable, so no warning.
        let mut r = healthy();
        r.http.requested = 80;
        r.http.bound = 8080;
        r.http.fell_back = true;
        r.port_redirect = Some(true);
        assert!(!codes(&diagnose(&r)).contains(&DiagnosisCode::PortFallback));

        // Redirect present but NOT active → still a warning.
        r.port_redirect = Some(false);
        assert!(codes(&diagnose(&r)).contains(&DiagnosisCode::PortFallback));

        // Not applicable (Linux, None) → unchanged warning behaviour.
        r.port_redirect = None;
        assert!(codes(&diagnose(&r)).contains(&DiagnosisCode::PortFallback));
    }

    #[test]
    fn foreign_web_listener_warns_and_suppresses_fallback() {
        // A privileged port fell back AND something foreign holds 80/443: show
        // the specific "another process" warning, not the (misleading) elevate
        // advice.
        let mut r = healthy();
        r.http.requested = 80;
        r.http.bound = 8080;
        r.http.fell_back = true;
        r.foreign_web_listener = Some(true);
        let cs = codes(&diagnose(&r));
        assert!(cs.contains(&DiagnosisCode::ForeignWebListener));
        assert!(
            !cs.contains(&DiagnosisCode::PortFallback),
            "foreign-listener finding supersedes the elevate-ports advice"
        );
        assert!(!cs.contains(&DiagnosisCode::AllGood));

        // No foreign listener (false/None) → the plain fallback warning returns.
        r.foreign_web_listener = Some(false);
        let cs = codes(&diagnose(&r));
        assert!(!cs.contains(&DiagnosisCode::ForeignWebListener));
        assert!(cs.contains(&DiagnosisCode::PortFallback));

        r.foreign_web_listener = None;
        assert!(codes(&diagnose(&r)).contains(&DiagnosisCode::PortFallback));
    }

    #[test]
    fn foreign_web_listener_warns_even_without_fallback() {
        // Even if the daemon reports ports bound, a foreign listener signal is
        // surfaced (e.g. the proxy lost the port to another process).
        let mut r = healthy();
        r.foreign_web_listener = Some(true);
        let cs = codes(&diagnose(&r));
        assert!(cs.contains(&DiagnosisCode::ForeignWebListener));
        assert!(!cs.contains(&DiagnosisCode::AllGood));
    }

    #[test]
    fn ca_and_resolver_unknown_is_silent() {
        let mut r = healthy();
        r.ca.trusted_system = None;
        r.resolver_installed = None;
        let cs = codes(&diagnose(&r));
        assert!(!cs.contains(&DiagnosisCode::CaNotTrusted));
        assert!(!cs.contains(&DiagnosisCode::ResolverNotInstalled));
    }

    #[test]
    fn ca_and_resolver_false_warns() {
        let mut r = healthy();
        r.ca.trusted_system = Some(false);
        r.resolver_installed = Some(false);
        let cs = codes(&diagnose(&r));
        assert!(cs.contains(&DiagnosisCode::CaNotTrusted));
        assert!(cs.contains(&DiagnosisCode::ResolverNotInstalled));
    }

    #[test]
    fn no_php_suppresses_default_not_installed() {
        let mut r = healthy();
        r.php.clear();
        let cs = codes(&diagnose(&r));
        assert!(cs.contains(&DiagnosisCode::NoPhpInstalled));
        assert!(!cs.contains(&DiagnosisCode::DefaultPhpNotInstalled));
    }

    #[test]
    fn default_not_installed_when_other_versions_present() {
        let mut r = healthy();
        // Installed 8.4, but default is 8.5.
        r.php[0].version = PhpVersion::new(8, 4);
        let cs = codes(&diagnose(&r));
        assert!(cs.contains(&DiagnosisCode::DefaultPhpNotInstalled));
        assert!(!cs.contains(&DiagnosisCode::NoPhpInstalled));
    }

    #[test]
    fn failed_pool_is_fail_and_auto_fixable() {
        let mut r = healthy();
        r.php[0].state = PoolRunState::Failed;
        let ds = diagnose(&r);
        assert!(codes(&ds).contains(&DiagnosisCode::FpmPoolFailed));
        assert!(ds
            .iter()
            .any(|d| d.code == DiagnosisCode::FpmPoolFailed && d.severity == Severity::Fail));
        assert_eq!(
            plan_auto_fixes(&r),
            vec![FixAction::RestartFpm(PhpVersion::new(8, 5))]
        );
        assert!(is_auto_fixable(DiagnosisCode::FpmPoolFailed));
        assert!(!is_auto_fixable(DiagnosisCode::CaNotTrusted));
    }

    #[test]
    fn update_available_is_informational_and_still_all_good() {
        let mut r = healthy();
        r.php[0].update_available = Some("8.5.7".into());
        let ds = diagnose(&r);
        let cs = codes(&ds);
        assert!(cs.contains(&DiagnosisCode::PhpUpdateAvailable));
        // Ok/info findings don't suppress the all-good summary.
        assert!(cs.contains(&DiagnosisCode::AllGood));
    }

    #[test]
    fn no_sites_is_informational() {
        let mut r = healthy();
        r.sites = SiteCounts::default();
        assert!(codes(&diagnose(&r)).contains(&DiagnosisCode::NoSites));
    }

    #[test]
    fn problems_suppress_all_good() {
        let mut r = healthy();
        r.ca.trusted_system = Some(false);
        assert!(!codes(&diagnose(&r)).contains(&DiagnosisCode::AllGood));
    }
}
