//! Daemon → client response envelope and error-code enum.
//!
//! Internally tagged on `type`, `snake_case`. Wire-stability assertions
//! live in `tests/wire_stability.rs`.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use yerd_core::{PhpVersion, Site};

use crate::dump::{DumpCounts, DumpEvent, DumpExtStatus};
use crate::status::{
    CloudflaredStatus, DatabaseSummary, Diagnosis, FixReport, MailDetail, MailSummary,
    NamedTunnelMeta, ServiceAvailability, ServiceStatus, SiteHostname, StatusReport, ToolStatus,
    TunnelInfo,
};

// Same rule: no per-field serde renames.
/// A response sent from the daemon to a client.
///
/// Not `Eq`: [`Response::Dumps`] carries [`DumpEvent`]s whose opaque
/// `serde_json::Value` payloads are only `PartialEq`. `PartialEq` is all the
/// wire-stability round-trips need.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Response {
    /// Reply to [`crate::Request::Ping`].
    Pong,
    /// Reply to [`crate::Request::ListSites`].
    Sites {
        /// The sites currently known to the daemon, in lexicographic
        /// name order.
        sites: Vec<SiteEntry>,
    },
    /// Generic success for mutating requests
    /// ([`crate::Request::Park`], [`crate::Request::Link`],
    /// [`crate::Request::Unlink`], [`crate::Request::SetPhp`],
    /// [`crate::Request::SetSecure`]).
    Ok,
    /// A request failed. `code` is machine-readable; `message` is for
    /// human display.
    Error {
        /// Machine-readable error category.
        code: ErrorCode,
        /// Human-readable error message.
        message: String,
    },
    /// Reply to [`crate::Request::ListParked`] - the registered parked roots,
    /// in lexicographic order (the daemon stores them in a `BTreeSet`).
    Parked {
        /// Canonical parked root paths.
        paths: Vec<String>,
    },
    /// Reply to [`crate::Request::DaemonInfo`] - read-only runtime facts.
    Info {
        /// Address the embedded DNS responder is bound on (`127.0.0.1:<port>`).
        dns_addr: SocketAddr,
        /// The TLD served (e.g. `"test"`).
        tld: String,
        /// Absolute path to the local CA certificate PEM.
        ca_path: PathBuf,
        /// SHA-256 fingerprint of the CA cert, 64 lowercase hex chars.
        ca_fingerprint: String,
        /// The rootless HTTP port the daemon actually bound (e.g. 8080). The
        /// macOS `yerd elevate ports` flow redirects 80 → this. `#[serde(default)]`
        /// keeps older daemons (which omit it) decodable; defaults to 0.
        #[serde(default)]
        http_port: u16,
        /// The rootless HTTPS port the daemon actually bound (e.g. 8443).
        #[serde(default)]
        https_port: u16,
        /// The configured rootless HTTP fallback port (e.g. 8080) - what Settings
        /// edits. Distinct from `http_port`, which is the *bound* port and equals
        /// the desired port when privileged binding succeeds. `#[serde(default)]`
        /// keeps older daemons decodable; defaults to 0.
        #[serde(default)]
        fallback_http: u16,
        /// The configured rootless HTTPS fallback port (e.g. 8443).
        #[serde(default)]
        fallback_https: u16,
        /// The configured DNS responder port (`dns_port`, e.g. 1053) - what
        /// Settings edits. Distinct from `dns_addr`, which is the *bound* address
        /// (and stays the wanted addr when the DNS port couldn't bind).
        /// `#[serde(default)]` keeps older daemons (which omit it) decodable;
        /// defaults to 0.
        #[serde(default)]
        dns_port: u16,
    },
    /// Reply to [`crate::Request::ListPhp`] / `CheckPhpUpdates` / `UpdatePhp`.
    PhpVersions {
        /// Installed versions, ascending.
        installed: Vec<PhpVersion>,
        /// The current global default.
        default: PhpVersion,
        /// Installed minors with a newer patch available (from the daemon's
        /// update cache). Empty when none / cache cold.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        updates: Vec<PhpUpdate>,
        /// Global PHP ini settings applied to every version's FPM pool
        /// (`"memory_limit" -> "512M"`). Empty when none are set.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        settings: BTreeMap<String, String>,
    },
    /// Reply to [`crate::Request::AvailablePhp`].
    AvailablePhp {
        /// Installable major.minor versions from the distribution, ascending.
        available: Vec<PhpVersion>,
        /// Currently installed versions, ascending, so clients can hide (GUI
        /// dropdown) or tag (CLI) them.
        installed: Vec<PhpVersion>,
    },
    /// Reply to [`crate::Request::Status`] - a runtime health snapshot.
    ///
    /// Boxed so the (large) report does not bloat every `Response` value;
    /// `Box<T>` serializes transparently, so the wire bytes are unchanged.
    Status {
        /// The assembled health report.
        report: Box<StatusReport>,
    },
    /// Reply to [`crate::Request::Diagnose`] - the doctor findings.
    Diagnoses {
        /// One entry per check that produced a finding.
        items: Vec<Diagnosis>,
    },
    /// Reply to [`crate::Request::DoctorFix`] - what was fixed + what remains.
    DoctorFix {
        /// The fix outcome.
        report: FixReport,
    },
    /// Reply to [`crate::Request::ListServices`].
    Services {
        /// One entry per manageable service.
        services: Vec<ServiceStatus>,
    },
    /// Reply to [`crate::Request::AvailableServices`].
    AvailableServices {
        /// Installable vs installed versions, per service.
        services: Vec<ServiceAvailability>,
    },
    /// Reply to [`crate::Request::ServiceLogs`] - trailing log lines, oldest first.
    ServiceLogs {
        /// The log lines.
        lines: Vec<String>,
    },
    /// Reply to [`crate::Request::ListDatabases`] - the user databases in a SQL
    /// service (system schemas filtered out).
    Databases {
        /// One entry per database, sorted by name.
        databases: Vec<DatabaseSummary>,
    },
    /// Reply to [`crate::Request::ListDumps`] - events newer than the client's
    /// cursor, the ids removed since then, the per-category counts, and the
    /// newest id currently buffered.
    Dumps {
        /// Events with `id > since_id`, oldest first.
        events: Vec<DumpEvent>,
        /// Ids deleted since the client's cursor, so it can drop stale rows it
        /// still holds. Best-effort (bounded log); `min_live_id` is the
        /// guaranteed lower bound for evicted/cleared rows.
        removed_ids: Vec<u64>,
        /// Current per-category counts of buffered events.
        counts: DumpCounts,
        /// The newest buffered event id (0 when the ring is empty); the client
        /// uses it as the next `since_id`.
        latest_id: u64,
        /// Smallest id still buffered. Clients drop any held id `< min_live_id`
        /// unconditionally - so dropping evicted/cleared rows never depends on
        /// the bounded `removed_ids` log.
        min_live_id: u64,
    },
    /// Reply to [`crate::Request::DumpsStatus`] - dump-server configuration and
    /// liveness.
    DumpsStatus {
        /// Whether dump interception is enabled (the antenna).
        enabled: bool,
        /// The loopback port the dump server listens on.
        port: u16,
        /// Whether the dump server is currently bound and accepting.
        running: bool,
        /// Whether log persistence is on (off = clear on each new request).
        persist: bool,
        /// Per-installed-version extension presence (a yerd-side fact).
        extensions: Vec<DumpExtStatus>,
        /// Current per-category counts of buffered events.
        counts: DumpCounts,
        /// Resolved per-feature capture flags (every key present; absent in
        /// config means on). `BTreeMap` for stable order.
        features: BTreeMap<String, bool>,
    },
    /// Reply to [`crate::Request::ListMails`] - captured email metadata, newest first.
    Mails {
        /// One entry per captured email.
        mails: Vec<MailSummary>,
    },
    /// Reply to [`crate::Request::GetMail`] - one captured email's full content.
    ///
    /// Boxed so the (large) `MailDetail` does not bloat every `Response` value -
    /// the same treatment as [`Self::Status`]. `Box<T>` serializes transparently,
    /// so the wire bytes are unchanged.
    Mail {
        /// The decoded email.
        mail: Box<MailDetail>,
    },
    /// Reply to [`crate::Request::ListTools`] - the installable dev tools.
    Tools {
        /// One entry per tool, with install status.
        tools: Vec<ToolStatus>,
    },
    /// Reply to [`crate::Request::CreateSite`] - the background job was started.
    JobStarted {
        /// The job id to poll with [`crate::Request::JobStatus`].
        job_id: crate::JobId,
    },
    /// Reply to [`crate::Request::JobStatus`] - a job's current progress.
    JobProgress {
        /// The job's lifecycle state.
        state: crate::JobState,
        /// A short human label for the current phase (e.g. `"Scaffolding"`).
        phase: String,
        /// Log lines newer than the client's cursor, oldest first.
        log: Vec<String>,
        /// The cursor the client should send on its next poll.
        next_cursor: u64,
        /// Set when `state` is [`crate::JobState::Failed`]: the failure message.
        error: Option<String>,
    },
    /// Reply to [`crate::Request::CheckUpdate`] - the running version, both
    /// channel latests, the active channel preference, and whether an update is
    /// available. Versions are strings (e.g. `"2.0.2-rc.3"`) to keep this crate
    /// free of a semver dependency.
    UpdateStatus {
        /// The running Yerd version.
        current: String,
        /// Highest stable version available, or `None` if none / unknown.
        latest_stable: Option<String>,
        /// Highest edge (pre-release-inclusive) version available, or `None`.
        latest_edge: Option<String>,
        /// The channel this check resolved against (the preference, unless
        /// overridden for this check).
        channel: crate::Channel,
        /// Whether a newer version is available on `channel`.
        available: bool,
        /// The version `channel` would update to (strictly newer than current),
        /// or `None` when already up to date.
        target: Option<String>,
        /// True when the running version is a pre-release ahead of the latest
        /// stable - switching to stable would be a downgrade.
        ahead_of_stable: bool,
        /// Whether these figures are from a live fetch or a cached fallback.
        source: crate::UpdateSource,
        /// Unix epoch (seconds) when this result was obtained, for a "last
        /// checked …" display. `None` when never checked (or an older daemon that
        /// predates the field). `#[serde(default, skip_serializing_if)]` keeps the
        /// wire additive.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checked_at_epoch: Option<u64>,
    },
    /// Reply to [`crate::Request::StageUpdate`] - the verified update artifact
    /// has been downloaded to `path`. The applier installs it.
    Staged {
        /// Absolute path to the verified, downloaded artifact on disk.
        path: String,
        /// The version that was staged (e.g. `"2.0.5"`).
        version: String,
        /// What kind of artifact it is (drives the applier's install method).
        kind: crate::StagedArtifact,
    },
    /// Reply to [`crate::Request::StartQuickTunnel`] / [`crate::Request::StopTunnel`]
    /// / [`crate::Request::TunnelStatus`] - the live tunnels plus `cloudflared`
    /// install status.
    Tunnels {
        /// One entry per live tunnel.
        tunnels: Vec<TunnelInfo>,
        /// `cloudflared` install / account status.
        cloudflared: CloudflaredStatus,
    },
    /// Reply to [`crate::Request::ListNamedTunnels`] - the account's named
    /// tunnels recorded locally, plus the per-site hostname mappings that are
    /// enabled in the consolidated tunnel.
    NamedTunnels {
        /// One entry per named tunnel.
        tunnels: Vec<NamedTunnelMeta>,
        /// The sites enabled in the named tunnel (site → hostname).
        sites: Vec<SiteHostname>,
        /// The authorized Cloudflare zone (domain) the account cert is scoped to,
        /// e.g. `"example.com"`. `None` when not logged in or unresolvable.
        /// `#[serde(default, skip_serializing_if)]` keeps the wire additive.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zone: Option<String>,
    },
    /// Reply to [`crate::Request::ListGroups`] - the user-defined site groups in
    /// display order and the per-site membership map (site name → group name).
    Groups {
        /// Group display names, in display order.
        order: Vec<String>,
        /// Per-site membership: site name → group name. A site absent from the
        /// map is ungrouped ("Unallocated").
        members: BTreeMap<String, String>,
    },
}

/// One entry in [`Response::Sites`]: the site plus WordPress-detection
/// metadata computed fresh at request time.
///
/// This is a wire-only wrapper, not a new field on [`Site`] itself - `Site`'s
/// hand-written `Serialize`/`Deserialize` is shared between the wire and
/// `yerd.toml` persistence (`Config.linked: Vec<Site>`), and `WordPress`
/// detection is a runtime fact (it can change the moment the user runs
/// `wp core update`), not something that belongs baked into persisted config.
/// `#[serde(flatten)]` keeps the JSON shape identical to "just add fields to
/// `Site`" from the wire's perspective without touching `Site`'s own shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteEntry {
    /// The site itself - unchanged shape, still exactly what `Site`'s own
    /// serde impl produces.
    #[serde(flatten)]
    pub site: Site,
    /// Whether a `WordPress` marker (`wp-config.php`/`wp-load.php`) was found
    /// at the site's served root.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_wordpress: bool,
    /// The installed `WordPress` core version, if `is_wordpress` and
    /// `wp-includes/version.php` parsed. Kept independent of `is_wordpress`
    /// rather than collapsed into one `Option<String>`: a genuine `WordPress`
    /// site whose version file doesn't parse (unusual layouts, a future core
    /// format change) must still show as `WordPress`, just without a version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wordpress_version: Option<String>,
}

/// An available newer patch for an installed PHP minor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhpUpdate {
    /// The installed minor (e.g. `8.5`).
    pub version: PhpVersion,
    /// The installed patch (e.g. `"8.5.6"`).
    pub installed: String,
    /// The newest published patch (e.g. `"8.5.7"`).
    pub latest: String,
}

/// Machine-readable error category for [`Response::Error`].
///
/// Fail-closed on unknown variants from a newer daemon (no
/// `#[serde(other)]` catch-all) - an unknown code surfaces as
/// [`crate::IpcError::Decode`], which is the broader "version mismatch
/// signal" until a `Hello`/`Welcome` handshake lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    /// The requested site or resource does not exist.
    NotFound,
    /// A site with that name is already registered.
    AlreadyExists,
    /// The supplied path was rejected (does not exist, not a
    /// directory, outside an allowed root, etc.).
    InvalidPath,
    /// A service's configured port is already in use by another listener.
    PortInUse,
    /// Catch-all for daemon-side failures that don't fit a typed code.
    /// Expand this enum rather than overloading `Internal`.
    Internal,
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    // The rename-trap match arms are deliberately all `{}`; merging
    // them would collapse the per-variant check that catches Rust
    // variant renames.
    clippy::match_same_arms
)]
mod variant_name_pinning {
    use super::*;

    #[allow(dead_code)]
    fn pin_response(r: Response) {
        match r {
            Response::Pong => {}
            Response::Sites { .. } => {}
            Response::Ok => {}
            Response::Error { .. } => {}
            Response::Parked { .. } => {}
            Response::Info { .. } => {}
            Response::PhpVersions { .. } => {}
            Response::AvailablePhp { .. } => {}
            Response::Status { .. } => {}
            Response::Diagnoses { .. } => {}
            Response::DoctorFix { .. } => {}
            Response::Services { .. } => {}
            Response::AvailableServices { .. } => {}
            Response::ServiceLogs { .. } => {}
            Response::Databases { .. } => {}
            Response::Dumps { .. } => {}
            Response::DumpsStatus { .. } => {}
            Response::Mails { .. } => {}
            Response::Mail { .. } => {}
            Response::Tools { .. } => {}
            Response::JobStarted { .. } => {}
            Response::JobProgress { .. } => {}
            Response::UpdateStatus { .. } => {}
            Response::Staged { .. } => {}
            Response::Tunnels { .. } => {}
            Response::NamedTunnels { .. } => {}
            Response::Groups { .. } => {}
        }
    }

    #[allow(dead_code)]
    fn pin_code(c: ErrorCode) {
        match c {
            ErrorCode::NotFound => {}
            ErrorCode::AlreadyExists => {}
            ErrorCode::InvalidPath => {}
            ErrorCode::PortInUse => {}
            ErrorCode::Internal => {}
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn touch_every_variant() {
        pin_response(Response::Pong);
        pin_response(Response::Sites { sites: vec![] });
        pin_response(Response::Ok);
        pin_response(Response::Error {
            code: ErrorCode::Internal,
            message: "x".into(),
        });
        pin_response(Response::Parked {
            paths: vec!["/x".into()],
        });
        pin_response(Response::Info {
            dns_addr: "127.0.0.1:1053".parse().unwrap(),
            tld: "test".into(),
            ca_path: PathBuf::from("/x/ca.cert.pem"),
            ca_fingerprint: "ab".repeat(32),
            http_port: 8080,
            https_port: 8443,
            fallback_http: 8080,
            fallback_https: 8443,
            dns_port: 1053,
        });
        pin_response(Response::PhpVersions {
            installed: vec![PhpVersion::new(8, 5)],
            default: PhpVersion::new(8, 5),
            updates: vec![],
            settings: BTreeMap::new(),
        });
        pin_response(Response::AvailablePhp {
            available: vec![PhpVersion::new(8, 4), PhpVersion::new(8, 5)],
            installed: vec![PhpVersion::new(8, 5)],
        });
        pin_response(Response::Status {
            report: Box::new(crate::status::StatusReport {
                daemon_pid: 1,
                uptime_secs: 0,
                daemon_rss_bytes: None,
                tld: "test".into(),
                http: crate::status::PortStatus {
                    requested: 80,
                    bound: 8080,
                    fell_back: true,
                },
                https: crate::status::PortStatus {
                    requested: 443,
                    bound: 8443,
                    fell_back: true,
                },
                dns_addr: "127.0.0.1:1053".parse().unwrap(),
                ca: crate::status::CaStatus {
                    path: PathBuf::from("/x/ca.cert.pem"),
                    fingerprint: "ab".repeat(32),
                    trusted_system: Some(false),
                    php_trusts_ca: None,
                },
                resolver_installed: None,
                port_redirect: None,
                foreign_web_listener: None,
                resolver_backup: None,
                default_php: PhpVersion::new(8, 5),
                php: vec![],
                sites: crate::status::SiteCounts::default(),
                load_avg: Some([100, 50, 25]),
                daemon_version: "9.9.9".into(),
                services: vec![],
                mail: None,
                web_unbound: None,
                dns_unbound: None,
                boot_id: None,
                shared_sites: 0,
            }),
        });
        pin_response(Response::Diagnoses {
            items: vec![crate::status::Diagnosis {
                code: crate::status::DiagnosisCode::AllGood,
                severity: crate::status::Severity::Ok,
                title: "x".into(),
                detail: "x".into(),
                remedy: None,
            }],
        });
        pin_response(Response::DoctorFix {
            report: crate::status::FixReport {
                performed: vec![],
                manual: vec![],
            },
        });
        pin_response(Response::Services { services: vec![] });
        pin_response(Response::AvailableServices { services: vec![] });
        pin_response(Response::ServiceLogs { lines: vec![] });
        pin_response(Response::Databases {
            databases: vec![DatabaseSummary { name: "app".into() }],
        });
        pin_response(Response::Dumps {
            events: vec![],
            removed_ids: vec![],
            counts: DumpCounts::default(),
            latest_id: 0,
            min_live_id: 0,
        });
        pin_response(Response::DumpsStatus {
            enabled: true,
            port: 2304,
            running: true,
            persist: false,
            extensions: vec![],
            counts: DumpCounts::default(),
            features: BTreeMap::new(),
        });
        pin_response(Response::Mails {
            mails: vec![crate::status::MailSummary {
                id: "000001".into(),
                from: "Example <hello@example.com>".into(),
                to: vec!["test@test.com".into()],
                subject: "Hi".into(),
                date_epoch: 1_700_000_000,
                read: false,
            }],
        });
        pin_response(Response::Mail {
            mail: Box::new(crate::status::MailDetail {
                id: "000001".into(),
                from: "Example <hello@example.com>".into(),
                to: vec!["test@test.com".into()],
                subject: "Hi".into(),
                date_epoch: 1_700_000_000,
                headers: vec![crate::status::MailHeader {
                    name: "Subject".into(),
                    value: "Hi".into(),
                }],
                html_body: Some("<p>Hi</p>".into()),
                text_body: Some("Hi".into()),
            }),
        });
        pin_response(Response::Tools {
            tools: vec![crate::status::ToolStatus {
                id: "node".into(),
                display_name: "Node.js".into(),
                installed: true,
                version: Some("v24.17.0".into()),
                binaries: vec!["node".into(), "npm".into(), "npx".into()],
                external: false,
                external_path: None,
            }],
        });
        pin_response(Response::JobStarted {
            job_id: "j1".into(),
        });
        pin_response(Response::JobProgress {
            state: crate::JobState::Running,
            phase: "Scaffolding".into(),
            log: vec!["line".into()],
            next_cursor: 1,
            error: None,
        });
        pin_response(Response::UpdateStatus {
            current: "2.0.2-rc.3".into(),
            latest_stable: Some("2.0.1".into()),
            latest_edge: Some("2.0.2-rc.3".into()),
            channel: crate::Channel::Stable,
            available: false,
            target: None,
            ahead_of_stable: true,
            source: crate::UpdateSource::Live,
            checked_at_epoch: Some(1_719_445_200),
        });
        pin_response(Response::Staged {
            path: "/x/Yerd.app.tar.gz".into(),
            version: "2.0.5".into(),
            kind: crate::StagedArtifact::AppTarGz,
        });
        pin_response(Response::Tunnels {
            tunnels: vec![crate::status::TunnelInfo {
                site: "app".into(),
                kind: crate::status::TunnelKind::Quick,
                state: crate::status::TunnelRunState::Running,
                url: Some("https://calm-river-1234.trycloudflare.com".into()),
                hostname: None,
            }],
            cloudflared: crate::status::CloudflaredStatus {
                installed: true,
                version: Some("2026.6.1".into()),
                source: Some(crate::status::CloudflaredSource::Managed),
                logged_in: false,
            },
        });
        pin_response(Response::NamedTunnels {
            tunnels: vec![crate::status::NamedTunnelMeta {
                name: "mysite".into(),
                uuid: "uuid-123".into(),
            }],
            sites: vec![crate::status::SiteHostname {
                site: "app".into(),
                hostname: "app.example.com".into(),
            }],
            zone: None,
        });
        pin_response(Response::Groups {
            order: vec!["Blog".into(), "Shop".into()],
            members: BTreeMap::from([("app".into(), "Blog".into())]),
        });
        for c in [
            ErrorCode::NotFound,
            ErrorCode::AlreadyExists,
            ErrorCode::InvalidPath,
            ErrorCode::PortInUse,
            ErrorCode::Internal,
        ] {
            pin_code(c);
        }
    }
}
