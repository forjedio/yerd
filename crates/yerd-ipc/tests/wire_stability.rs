//! Byte-exact wire-stability assertions for every `Request`,
//! `Response`, and `ErrorCode` variant.
//!
//! These literals are the published contract. A rename, reorder, or
//! casing change of any field or variant fails this file — which
//! fails CI before any downstream client sees a divergent wire format.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::disallowed_names
)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use yerd_ipc::{
    types::{PhpVersion, Site},
    CaStatus, Channel, DatabaseSummary, Diagnosis, DiagnosisCode, DumpCategory, DumpCounts,
    DumpEvent, DumpExtStatus, ErrorCode, FixReport, FixResult, MailDetail, MailHeader, MailStatus,
    MailSummary, PhpPoolStatus, PoolRunState, PortStatus, Request, Response, ServiceAvailability,
    ServiceRunState, ServiceStatus, Severity, SiteCounts, StagedArtifact, StatusReport, ToolStatus,
    UpdateSource,
};

// ---------- Request ----------

#[test]
fn request_ping_byte_shape() {
    let s = serde_json::to_string(&Request::Ping).unwrap();
    assert_eq!(s, r#"{"type":"ping"}"#);
    let back: Request = serde_json::from_str(&s).unwrap();
    assert_eq!(back, Request::Ping);
}

#[test]
fn request_list_sites_byte_shape() {
    let s = serde_json::to_string(&Request::ListSites).unwrap();
    assert_eq!(s, r#"{"type":"list_sites"}"#);
    let back: Request = serde_json::from_str(&s).unwrap();
    assert_eq!(back, Request::ListSites);
}

#[test]
fn request_park_byte_shape() {
    let r = Request::Park {
        path: PathBuf::from("/srv/foo"),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"park","path":"/srv/foo"}"#);
    let back: Request = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn request_link_byte_shape() {
    let r = Request::Link {
        name: "foo".into(),
        path: PathBuf::from("/srv/foo"),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"link","name":"foo","path":"/srv/foo"}"#);
    let back: Request = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn request_unlink_byte_shape() {
    let r = Request::Unlink { name: "foo".into() };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"unlink","name":"foo"}"#);
    let back: Request = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn request_list_parked_byte_shape() {
    let s = serde_json::to_string(&Request::ListParked).unwrap();
    assert_eq!(s, r#"{"type":"list_parked"}"#);
    let back: Request = serde_json::from_str(&s).unwrap();
    assert_eq!(back, Request::ListParked);
}

#[test]
fn request_unpark_byte_shape() {
    let r = Request::Unpark {
        path: "/srv/sites".into(),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"unpark","path":"/srv/sites"}"#);
    let back: Request = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn request_set_php_byte_shape() {
    let r = Request::SetPhp {
        name: "foo".into(),
        version: PhpVersion::new(8, 3),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"set_php","name":"foo","version":"8.3"}"#);
    let back: Request = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn request_set_secure_byte_shape() {
    let r = Request::SetSecure {
        name: "foo".into(),
        secure: true,
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"set_secure","name":"foo","secure":true}"#);
    let back: Request = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn request_set_web_root_byte_shape() {
    let some = Request::SetWebRoot {
        name: "foo".into(),
        path: Some("public".into()),
    };
    let s = serde_json::to_string(&some).unwrap();
    assert_eq!(s, r#"{"type":"set_web_root","name":"foo","path":"public"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), some);

    let none = Request::SetWebRoot {
        name: "foo".into(),
        path: None,
    };
    let s = serde_json::to_string(&none).unwrap();
    assert_eq!(s, r#"{"type":"set_web_root","name":"foo","path":null}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), none);
}

#[test]
fn request_daemon_info_byte_shape() {
    let s = serde_json::to_string(&Request::DaemonInfo).unwrap();
    assert_eq!(s, r#"{"type":"daemon_info"}"#);
    let back: Request = serde_json::from_str(&s).unwrap();
    assert_eq!(back, Request::DaemonInfo);
}

#[test]
fn request_install_php_byte_shape() {
    let r = Request::InstallPhp {
        version: PhpVersion::new(8, 5),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"install_php","version":"8.5"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_set_default_php_byte_shape() {
    let r = Request::SetDefaultPhp {
        version: PhpVersion::new(8, 4),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"set_default_php","version":"8.4"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_list_php_byte_shape() {
    let s = serde_json::to_string(&Request::ListPhp).unwrap();
    assert_eq!(s, r#"{"type":"list_php"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&s).unwrap(),
        Request::ListPhp
    );
}

#[test]
fn request_update_php_byte_shape() {
    let some = Request::UpdatePhp {
        version: Some(PhpVersion::new(8, 5)),
    };
    let s = serde_json::to_string(&some).unwrap();
    assert_eq!(s, r#"{"type":"update_php","version":"8.5"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), some);

    let all = Request::UpdatePhp { version: None };
    let s = serde_json::to_string(&all).unwrap();
    assert_eq!(s, r#"{"type":"update_php","version":null}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), all);
}

#[test]
fn request_check_php_updates_byte_shape() {
    let s = serde_json::to_string(&Request::CheckPhpUpdates).unwrap();
    assert_eq!(s, r#"{"type":"check_php_updates"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&s).unwrap(),
        Request::CheckPhpUpdates
    );
}

#[test]
fn request_available_php_byte_shape() {
    let s = serde_json::to_string(&Request::AvailablePhp).unwrap();
    assert_eq!(s, r#"{"type":"available_php"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&s).unwrap(),
        Request::AvailablePhp
    );
}

#[test]
fn request_restart_php_byte_shape() {
    let r = Request::RestartPhp {
        version: PhpVersion::new(8, 3),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"restart_php","version":"8.3"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_restart_all_php_byte_shape() {
    let s = serde_json::to_string(&Request::RestartAllPhp).unwrap();
    assert_eq!(s, r#"{"type":"restart_all_php"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&s).unwrap(),
        Request::RestartAllPhp
    );
}

#[test]
fn request_uninstall_php_byte_shape() {
    let r = Request::UninstallPhp {
        version: PhpVersion::new(8, 3),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"uninstall_php","version":"8.3"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_status_byte_shape() {
    let s = serde_json::to_string(&Request::Status).unwrap();
    assert_eq!(s, r#"{"type":"status"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&s).unwrap(),
        Request::Status
    );
}

#[test]
fn request_diagnose_byte_shape() {
    let s = serde_json::to_string(&Request::Diagnose).unwrap();
    assert_eq!(s, r#"{"type":"diagnose"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&s).unwrap(),
        Request::Diagnose
    );
}

#[test]
fn request_restart_daemon_byte_shape() {
    let s = serde_json::to_string(&Request::RestartDaemon).unwrap();
    assert_eq!(s, r#"{"type":"restart_daemon"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&s).unwrap(),
        Request::RestartDaemon
    );
}

#[test]
fn request_doctor_fix_byte_shape() {
    let s = serde_json::to_string(&Request::DoctorFix).unwrap();
    assert_eq!(s, r#"{"type":"doctor_fix"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&s).unwrap(),
        Request::DoctorFix
    );
}

// ---------- Response ----------

#[test]
fn response_pong_byte_shape() {
    let s = serde_json::to_string(&Response::Pong).unwrap();
    assert_eq!(s, r#"{"type":"pong"}"#);
    let back: Response = serde_json::from_str(&s).unwrap();
    assert_eq!(back, Response::Pong);
}

#[test]
fn response_ok_byte_shape() {
    let s = serde_json::to_string(&Response::Ok).unwrap();
    assert_eq!(s, r#"{"type":"ok"}"#);
    let back: Response = serde_json::from_str(&s).unwrap();
    assert_eq!(back, Response::Ok);
}

#[test]
fn response_sites_zero_byte_shape() {
    let r = Response::Sites { sites: vec![] };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"sites","sites":[]}"#);
    let back: Response = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn response_sites_one_byte_shape() {
    let foo = Site::parked("foo", "/srv/foo", PhpVersion::new(8, 3)).unwrap();
    let r = Response::Sites { sites: vec![foo] };
    let s = serde_json::to_string(&r).unwrap();
    let expected = r#"{"type":"sites","sites":[{"name":"foo","document_root":"/srv/foo","php":"8.3","secure":false,"kind":"parked"}]}"#;
    assert_eq!(s, expected);
    let back: Response = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn response_sites_two_byte_shape() {
    let alpha = Site::parked("alpha", "/srv/alpha", PhpVersion::new(8, 3)).unwrap();
    // beta must call set_secure(true) explicitly — constructors
    // initialise secure=false.
    let mut beta = Site::linked("beta", "/srv/beta", PhpVersion::new(7, 4)).unwrap();
    beta.set_secure(true);
    let r = Response::Sites {
        sites: vec![alpha, beta],
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected = r#"{"type":"sites","sites":[{"name":"alpha","document_root":"/srv/alpha","php":"8.3","secure":false,"kind":"parked"},{"name":"beta","document_root":"/srv/beta","php":"7.4","secure":true,"kind":"linked"}]}"#;
    assert_eq!(s, expected);
    let back: Response = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn response_sites_with_web_subpath_byte_shape() {
    // A site with a non-empty web_subpath emits the key right after
    // document_root; empty subpaths are omitted (pinned by the tests above).
    let mut app = Site::linked("app", "/srv/app", PhpVersion::new(8, 3)).unwrap();
    app.set_web_subpath("public");
    let r = Response::Sites { sites: vec![app] };
    let s = serde_json::to_string(&r).unwrap();
    let expected = r#"{"type":"sites","sites":[{"name":"app","document_root":"/srv/app","web_subpath":"public","php":"8.3","secure":false,"kind":"linked"}]}"#;
    assert_eq!(s, expected);
    let back: Response = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn response_parked_byte_shape() {
    let r = Response::Parked {
        paths: vec!["/a".into(), "/b".into()],
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"parked","paths":["/a","/b"]}"#);
    let back: Response = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn response_parked_empty_byte_shape() {
    let r = Response::Parked { paths: vec![] };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"parked","paths":[]}"#);
    let back: Response = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn response_info_byte_shape() {
    let r = Response::Info {
        dns_addr: "127.0.0.1:1053".parse().unwrap(),
        tld: "test".into(),
        ca_path: std::path::PathBuf::from("/home/u/.local/share/yerd/ca.cert.pem"),
        ca_fingerprint: "ab".repeat(32),
        http_port: 8080,
        https_port: 8443,
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected = format!(
        r#"{{"type":"info","dns_addr":"127.0.0.1:1053","tld":"test","ca_path":"/home/u/.local/share/yerd/ca.cert.pem","ca_fingerprint":"{}","http_port":8080,"https_port":8443}}"#,
        "ab".repeat(32)
    );
    assert_eq!(s, expected);
    let back: Response = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);

    // Back-compat: an older daemon omits the port fields; they default to 0.
    let legacy = format!(
        r#"{{"type":"info","dns_addr":"127.0.0.1:1053","tld":"test","ca_path":"/x","ca_fingerprint":"{}"}}"#,
        "ab".repeat(32)
    );
    let decoded: Response = serde_json::from_str(&legacy).unwrap();
    assert!(matches!(
        decoded,
        Response::Info {
            http_port: 0,
            https_port: 0,
            ..
        }
    ));
}

#[test]
fn response_php_versions_byte_shape() {
    // Empty `updates` is skipped on the wire → same bytes as before the field
    // was added (the round-trip restores it to an empty Vec via `default`).
    let r = Response::PhpVersions {
        installed: vec![PhpVersion::new(8, 3), PhpVersion::new(8, 5)],
        default: PhpVersion::new(8, 5),
        updates: vec![],
        settings: BTreeMap::new(),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"php_versions","installed":["8.3","8.5"],"default":"8.5"}"#
    );
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn response_php_versions_with_updates_byte_shape() {
    let r = Response::PhpVersions {
        installed: vec![PhpVersion::new(8, 5)],
        default: PhpVersion::new(8, 5),
        updates: vec![yerd_ipc::PhpUpdate {
            version: PhpVersion::new(8, 5),
            installed: "8.5.6".into(),
            latest: "8.5.7".into(),
        }],
        settings: BTreeMap::new(),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"php_versions","installed":["8.5"],"default":"8.5","updates":[{"version":"8.5","installed":"8.5.6","latest":"8.5.7"}]}"#
    );
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn response_php_versions_with_settings_byte_shape() {
    // Non-empty `settings` is appended after `updates`; BTreeMap keys are
    // serialised in sorted order.
    let r = Response::PhpVersions {
        installed: vec![PhpVersion::new(8, 5)],
        default: PhpVersion::new(8, 5),
        updates: vec![],
        settings: BTreeMap::from([
            ("memory_limit".to_string(), "512M".to_string()),
            ("display_errors".to_string(), "On".to_string()),
        ]),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"php_versions","installed":["8.5"],"default":"8.5","settings":{"display_errors":"On","memory_limit":"512M"}}"#
    );
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn request_set_php_settings_byte_shape() {
    let empty = Request::SetPhpSettings {
        settings: BTreeMap::new(),
    };
    let s = serde_json::to_string(&empty).unwrap();
    assert_eq!(s, r#"{"type":"set_php_settings","settings":{}}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), empty);

    let populated = Request::SetPhpSettings {
        settings: BTreeMap::from([
            ("memory_limit".to_string(), "512M".to_string()),
            ("max_execution_time".to_string(), "30".to_string()),
        ]),
    };
    let s = serde_json::to_string(&populated).unwrap();
    assert_eq!(
        s,
        r#"{"type":"set_php_settings","settings":{"max_execution_time":"30","memory_limit":"512M"}}"#
    );
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), populated);
}

#[test]
fn response_available_php_byte_shape() {
    let r = Response::AvailablePhp {
        available: vec![PhpVersion::new(8, 4), PhpVersion::new(8, 5)],
        installed: vec![PhpVersion::new(8, 5)],
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"available_php","available":["8.4","8.5"],"installed":["8.5"]}"#
    );
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn response_error_each_code_byte_shape() {
    for (code, text) in [
        (ErrorCode::NotFound, "not_found"),
        (ErrorCode::AlreadyExists, "already_exists"),
        (ErrorCode::InvalidPath, "invalid_path"),
        (ErrorCode::Internal, "internal"),
    ] {
        let r = Response::Error {
            code,
            message: "x".into(),
        };
        let s = serde_json::to_string(&r).unwrap();
        let expected = format!(r#"{{"type":"error","code":"{text}","message":"x"}}"#);
        assert_eq!(s, expected, "code = {code:?}");
        let back: Response = serde_json::from_str(&s).unwrap();
        assert_eq!(back, r, "code = {code:?}");
    }
}

#[test]
fn response_status_byte_shape() {
    let r = Response::Status {
        report: Box::new(StatusReport {
            daemon_pid: 4242,
            uptime_secs: 7,
            daemon_rss_bytes: Some(2048),
            tld: "test".into(),
            http: PortStatus {
                requested: 80,
                bound: 8080,
                fell_back: true,
            },
            https: PortStatus {
                requested: 443,
                bound: 8443,
                fell_back: true,
            },
            dns_addr: "127.0.0.1:1053".parse().unwrap(),
            ca: CaStatus {
                path: PathBuf::from("/x/ca.cert.pem"),
                fingerprint: "ab".repeat(32),
                trusted_system: Some(false),
            },
            resolver_installed: Some(true),
            // `None` + skip_serializing_if → omitted from the wire, so the
            // Linux byte shape is unchanged from before the field existed.
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
                linked: 2,
                secured: 1,
            },
            load_avg: Some([100, 50, 25]),
            daemon_version: "2.0.1".into(),
            services: vec![],
            // `None` + skip_serializing_if → omitted, so the byte shape is
            // unchanged from before the mail field existed.
            mail: None,
        }),
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected = format!(
        r#"{{"type":"status","report":{{"daemon_pid":4242,"uptime_secs":7,"daemon_rss_bytes":2048,"tld":"test","http":{{"requested":80,"bound":8080,"fell_back":true}},"https":{{"requested":443,"bound":8443,"fell_back":true}},"dns_addr":"127.0.0.1:1053","ca":{{"path":"/x/ca.cert.pem","fingerprint":"{}","trusted_system":false}},"resolver_installed":true,"default_php":"8.5","php":[{{"version":"8.5","installed_patch":"8.5.6","state":"running","pid":99,"listen":"/run/fpm.sock","rss_bytes":1024,"update_available":null}}],"sites":{{"parked":1,"linked":2,"secured":1}},"load_avg":[100,50,25],"daemon_version":"2.0.1"}}}}"#,
        "ab".repeat(32)
    );
    assert_eq!(s, expected);
    let back: Response = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn status_port_redirect_appears_only_when_some() {
    // When set (macOS), `port_redirect` serializes right after
    // `resolver_installed`; when `None` it is omitted entirely.
    let mut report = sample_status_report();
    report.port_redirect = Some(true);
    let s = serde_json::to_string(&report).unwrap();
    assert!(
        s.contains(r#""resolver_installed":true,"port_redirect":true"#),
        "{s}"
    );

    report.port_redirect = None;
    let s = serde_json::to_string(&report).unwrap();
    assert!(!s.contains("port_redirect"), "{s}");
}

#[test]
fn status_foreign_web_listener_appears_only_when_some() {
    // When set, `foreign_web_listener` serializes between `port_redirect` and
    // `resolver_backup`; when `None` it is omitted entirely (additive wire).
    let mut report = sample_status_report();
    report.port_redirect = Some(true);
    report.foreign_web_listener = Some(true);
    let s = serde_json::to_string(&report).unwrap();
    assert!(
        s.contains(r#""port_redirect":true,"foreign_web_listener":true"#),
        "{s}"
    );

    report.foreign_web_listener = None;
    let s = serde_json::to_string(&report).unwrap();
    assert!(!s.contains("foreign_web_listener"), "{s}");
}

#[test]
fn status_resolver_backup_appears_only_when_some() {
    // When set (macOS), `resolver_backup` serializes right after
    // `port_redirect`; when `None` it is omitted entirely.
    let mut report = sample_status_report();
    report.port_redirect = Some(true);
    report.resolver_backup =
        Some("/Library/Application Support/io.yerd.Yerd/resolver-backups/test-1.conf".to_owned());
    let s = serde_json::to_string(&report).unwrap();
    assert!(
        s.contains(r#""port_redirect":true,"resolver_backup":"/Library"#),
        "{s}"
    );

    report.resolver_backup = None;
    let s = serde_json::to_string(&report).unwrap();
    assert!(!s.contains("resolver_backup"), "{s}");
}

/// A minimal healthy report for field-presence assertions.
fn sample_status_report() -> StatusReport {
    StatusReport {
        daemon_pid: 1,
        uptime_secs: 0,
        daemon_rss_bytes: None,
        tld: "test".into(),
        http: PortStatus {
            requested: 80,
            bound: 8080,
            fell_back: true,
        },
        https: PortStatus {
            requested: 443,
            bound: 8443,
            fell_back: true,
        },
        dns_addr: "127.0.0.1:1053".parse().unwrap(),
        ca: CaStatus {
            path: PathBuf::from("/x/ca.cert.pem"),
            fingerprint: "ab".repeat(32),
            trusted_system: Some(true),
        },
        resolver_installed: Some(true),
        port_redirect: None,
        foreign_web_listener: None,
        resolver_backup: None,
        default_php: PhpVersion::new(8, 5),
        php: vec![],
        sites: SiteCounts::default(),
        load_avg: None,
        daemon_version: "2.0.1".into(),
        services: vec![],
        mail: None,
    }
}

#[test]
fn status_services_appear_only_when_non_empty() {
    // Empty services → omitted (additive: byte shape unchanged from before the
    // field existed). Non-empty → appended after `daemon_version`.
    let mut report = sample_status_report();
    let s = serde_json::to_string(&report).unwrap();
    assert!(
        !s.contains("services"),
        "empty services must be omitted: {s}"
    );

    report.services = vec![ServiceStatus {
        service: "redis".into(),
        display_name: "Redis (Valkey)".into(),
        installed_versions: vec!["8".into()],
        selected_version: Some("8".into()),
        state: ServiceRunState::Running,
        pid: Some(42),
        listen: Some("127.0.0.1:6379".into()),
        port: 6379,
        enabled: true,
        supports_databases: false,
    }];
    let s = serde_json::to_string(&report).unwrap();
    assert!(
        s.contains(r#""daemon_version":"2.0.1","services":[{"service":"redis""#),
        "{s}"
    );
    let back: StatusReport = serde_json::from_str(&s).unwrap();
    assert_eq!(back, report);
}

#[test]
fn response_diagnoses_byte_shape() {
    let r = Response::Diagnoses {
        items: vec![Diagnosis {
            code: DiagnosisCode::PortFallback,
            severity: Severity::Warn,
            title: "t".into(),
            detail: "d".into(),
            remedy: Some("sudo yerd elevate ports".into()),
        }],
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"diagnoses","items":[{"code":"port_fallback","severity":"warn","title":"t","detail":"d","remedy":"sudo yerd elevate ports"}]}"#
    );
    let back: Response = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn response_doctor_fix_byte_shape() {
    let r = Response::DoctorFix {
        report: FixReport {
            performed: vec![FixResult {
                code: DiagnosisCode::FpmPoolFailed,
                ok: true,
                message: "restarted 8.5".into(),
            }],
            manual: vec![Diagnosis {
                code: DiagnosisCode::CaNotTrusted,
                severity: Severity::Warn,
                title: "t".into(),
                detail: "d".into(),
                remedy: Some("sudo yerd elevate trust".into()),
            }],
        },
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"doctor_fix","report":{"performed":[{"code":"fpm_pool_failed","ok":true,"message":"restarted 8.5"}],"manual":[{"code":"ca_not_trusted","severity":"warn","title":"t","detail":"d","remedy":"sudo yerd elevate trust"}]}}"#
    );
    let back: Response = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn pool_run_state_each_variant_byte_shape() {
    for (st, expected) in [
        (PoolRunState::Running, r#""running""#),
        (PoolRunState::Stopped, r#""stopped""#),
        (PoolRunState::Failed, r#""failed""#),
    ] {
        assert_eq!(serde_json::to_string(&st).unwrap(), expected);
    }
}

#[test]
fn severity_each_variant_byte_shape() {
    for (sv, expected) in [
        (Severity::Ok, r#""ok""#),
        (Severity::Warn, r#""warn""#),
        (Severity::Fail, r#""fail""#),
    ] {
        assert_eq!(serde_json::to_string(&sv).unwrap(), expected);
    }
}

#[test]
fn diagnosis_code_each_variant_byte_shape() {
    let cases: &[(DiagnosisCode, &str)] = &[
        (DiagnosisCode::DaemonDown, r#""daemon_down""#),
        (DiagnosisCode::PortFallback, r#""port_fallback""#),
        (
            DiagnosisCode::ForeignWebListener,
            r#""foreign_web_listener""#,
        ),
        (DiagnosisCode::CaNotTrusted, r#""ca_not_trusted""#),
        (
            DiagnosisCode::ResolverNotInstalled,
            r#""resolver_not_installed""#,
        ),
        (DiagnosisCode::NoPhpInstalled, r#""no_php_installed""#),
        (
            DiagnosisCode::DefaultPhpNotInstalled,
            r#""default_php_not_installed""#,
        ),
        (DiagnosisCode::FpmPoolFailed, r#""fpm_pool_failed""#),
        (
            DiagnosisCode::PhpUpdateAvailable,
            r#""php_update_available""#,
        ),
        (DiagnosisCode::NoSites, r#""no_sites""#),
        (
            DiagnosisCode::ResolverBackupSaved,
            r#""resolver_backup_saved""#,
        ),
        (DiagnosisCode::ServiceFailed, r#""service_failed""#),
        (DiagnosisCode::BinDirNotOnPath, r#""bin_dir_not_on_path""#),
        (DiagnosisCode::AllGood, r#""all_good""#),
    ];
    for (code, expected) in cases {
        assert_eq!(&serde_json::to_string(code).unwrap(), expected, "{code:?}");
    }
}

// ---------- ErrorCode (standalone) ----------

#[test]
fn error_code_each_variant_byte_shape() {
    let cases: &[(ErrorCode, &str)] = &[
        (ErrorCode::NotFound, r#""not_found""#),
        (ErrorCode::AlreadyExists, r#""already_exists""#),
        (ErrorCode::InvalidPath, r#""invalid_path""#),
        (ErrorCode::PortInUse, r#""port_in_use""#),
        (ErrorCode::Internal, r#""internal""#),
    ];
    for (code, expected) in cases {
        let s = serde_json::to_string(code).unwrap();
        assert_eq!(&s, expected, "code = {code:?}");
        let back: ErrorCode = serde_json::from_str(&s).unwrap();
        assert_eq!(back, *code);
    }
}

// ---------- Services (request + response) ----------

#[test]
fn request_list_services_byte_shape() {
    let s = serde_json::to_string(&Request::ListServices).unwrap();
    assert_eq!(s, r#"{"type":"list_services"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&s).unwrap(),
        Request::ListServices
    );
}

#[test]
fn request_available_services_byte_shape() {
    let s = serde_json::to_string(&Request::AvailableServices).unwrap();
    assert_eq!(s, r#"{"type":"available_services"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&s).unwrap(),
        Request::AvailableServices
    );
}

#[test]
fn request_install_service_byte_shape() {
    let r = Request::InstallService {
        service: "redis".into(),
        version: "8".into(),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"install_service","service":"redis","version":"8"}"#
    );
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_uninstall_service_byte_shape() {
    let r = Request::UninstallService {
        service: "redis".into(),
        version: "8".into(),
        purge: true,
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"uninstall_service","service":"redis","version":"8","purge":true}"#
    );
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_start_stop_restart_service_byte_shape() {
    let start = Request::StartService {
        service: "redis".into(),
    };
    assert_eq!(
        serde_json::to_string(&start).unwrap(),
        r#"{"type":"start_service","service":"redis"}"#
    );
    let stop = Request::StopService {
        service: "redis".into(),
    };
    assert_eq!(
        serde_json::to_string(&stop).unwrap(),
        r#"{"type":"stop_service","service":"redis"}"#
    );
    let restart = Request::RestartService {
        service: "redis".into(),
    };
    assert_eq!(
        serde_json::to_string(&restart).unwrap(),
        r#"{"type":"restart_service","service":"redis"}"#
    );
    for r in [start, stop, restart] {
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
    }
}

#[test]
fn request_set_service_port_byte_shape() {
    let r = Request::SetServicePort {
        service: "redis".into(),
        port: 6380,
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"set_service_port","service":"redis","port":6380}"#
    );
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_service_logs_byte_shape() {
    let r = Request::ServiceLogs {
        service: "redis".into(),
        lines: 100,
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"service_logs","service":"redis","lines":100}"#
    );
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_create_database_byte_shape() {
    let r = Request::CreateDatabase {
        service: "mysql".into(),
        name: "app".into(),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"create_database","service":"mysql","name":"app"}"#
    );
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_change_service_version_byte_shape() {
    let r = Request::ChangeServiceVersion {
        service: "redis".into(),
        version: "9.1.0".into(),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"change_service_version","service":"redis","version":"9.1.0"}"#
    );
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_list_databases_byte_shape() {
    let r = Request::ListDatabases {
        service: "mysql".into(),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"list_databases","service":"mysql"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_drop_database_byte_shape() {
    let r = Request::DropDatabase {
        service: "mysql".into(),
        name: "app".into(),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"drop_database","service":"mysql","name":"app"}"#
    );
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_backup_database_byte_shape() {
    let r = Request::BackupDatabase {
        service: "mysql".into(),
        name: "app".into(),
        path: PathBuf::from("/srv/app.sql"),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"backup_database","service":"mysql","name":"app","path":"/srv/app.sql"}"#
    );
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_restore_database_byte_shape() {
    let r = Request::RestoreDatabase {
        service: "mysql".into(),
        name: "app".into(),
        path: PathBuf::from("/srv/app.sql"),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"restore_database","service":"mysql","name":"app","path":"/srv/app.sql"}"#
    );
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn response_databases_byte_shape() {
    let r = Response::Databases {
        databases: vec![
            DatabaseSummary { name: "app".into() },
            DatabaseSummary {
                name: "blog".into(),
            },
        ],
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"databases","databases":[{"name":"app"},{"name":"blog"}]}"#
    );
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn response_services_byte_shape() {
    let r = Response::Services {
        services: vec![ServiceStatus {
            service: "redis".into(),
            display_name: "Redis (Valkey)".into(),
            installed_versions: vec!["8".into()],
            selected_version: Some("8".into()),
            state: ServiceRunState::Running,
            pid: Some(42),
            listen: Some("127.0.0.1:6379".into()),
            port: 6379,
            enabled: true,
            supports_databases: false,
        }],
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected = r#"{"type":"services","services":[{"service":"redis","display_name":"Redis (Valkey)","installed_versions":["8"],"selected_version":"8","state":"running","pid":42,"listen":"127.0.0.1:6379","port":6379,"enabled":true,"supports_databases":false}]}"#;
    assert_eq!(s, expected);
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn response_services_empty_byte_shape() {
    let r = Response::Services { services: vec![] };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"services","services":[]}"#);
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn response_available_services_byte_shape() {
    let r = Response::AvailableServices {
        services: vec![ServiceAvailability {
            service: "redis".into(),
            available: vec!["7".into(), "8".into()],
            installed: vec!["8".into()],
        }],
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected = r#"{"type":"available_services","services":[{"service":"redis","available":["7","8"],"installed":["8"]}]}"#;
    assert_eq!(s, expected);
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn response_service_logs_byte_shape() {
    let r = Response::ServiceLogs {
        lines: vec!["starting".into(), "ready".into()],
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"service_logs","lines":["starting","ready"]}"#);
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn service_run_state_each_variant_byte_shape() {
    for (st, expected) in [
        (ServiceRunState::Running, r#""running""#),
        (ServiceRunState::Stopped, r#""stopped""#),
        (ServiceRunState::Failed, r#""failed""#),
    ] {
        assert_eq!(serde_json::to_string(&st).unwrap(), expected);
    }
}

// ---------- Dumps ----------

#[test]
fn request_list_dumps_byte_shape() {
    let r = Request::ListDumps { since_id: 0 };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"list_dumps","since_id":0}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_clear_dumps_byte_shape() {
    let s = serde_json::to_string(&Request::ClearDumps).unwrap();
    assert_eq!(s, r#"{"type":"clear_dumps"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&s).unwrap(),
        Request::ClearDumps
    );
}

#[test]
fn request_delete_dump_byte_shape() {
    let r = Request::DeleteDump { id: 7 };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"delete_dump","id":7}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_set_dumps_enabled_byte_shape() {
    let r = Request::SetDumpsEnabled { enabled: true };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"set_dumps_enabled","enabled":true}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_set_dumps_port_byte_shape() {
    let r = Request::SetDumpsPort { port: 2304 };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"set_dumps_port","port":2304}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_set_dump_feature_byte_shape() {
    let r = Request::SetDumpFeature {
        feature: "queries".into(),
        enabled: false,
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(
        s,
        r#"{"type":"set_dump_feature","feature":"queries","enabled":false}"#
    );
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_set_dumps_persist_byte_shape() {
    let r = Request::SetDumpsPersist { persist: true };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"set_dumps_persist","persist":true}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_dumps_status_byte_shape() {
    let s = serde_json::to_string(&Request::DumpsStatus).unwrap();
    assert_eq!(s, r#"{"type":"dumps_status"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&s).unwrap(),
        Request::DumpsStatus
    );
}

#[test]
fn dump_category_each_variant_byte_shape() {
    for (c, expected) in [
        (DumpCategory::Dump, r#""dump""#),
        (DumpCategory::Query, r#""query""#),
        (DumpCategory::Job, r#""job""#),
        (DumpCategory::View, r#""view""#),
        (DumpCategory::Request, r#""request""#),
        (DumpCategory::Log, r#""log""#),
        (DumpCategory::Cache, r#""cache""#),
        (DumpCategory::Http, r#""http""#),
    ] {
        assert_eq!(serde_json::to_string(&c).unwrap(), expected);
    }
}

#[test]
fn dump_counts_byte_shape() {
    let c = DumpCounts::default();
    let s = serde_json::to_string(&c).unwrap();
    assert_eq!(
        s,
        r#"{"dumps":0,"queries":0,"jobs":0,"views":0,"requests":0,"logs":0,"cache":0,"http":0}"#
    );
    assert_eq!(serde_json::from_str::<DumpCounts>(&s).unwrap(), c);
}

#[test]
fn dump_event_byte_shape() {
    let e = DumpEvent {
        id: 1,
        category: DumpCategory::Query,
        ts_ms: 1_718_360_452_123,
        site: "blog.test".into(),
        request_id: "abc".into(),
        payload: serde_json::json!({ "sql": "select 1" }),
    };
    let s = serde_json::to_string(&e).unwrap();
    let expected = r#"{"id":1,"category":"query","ts_ms":1718360452123,"site":"blog.test","request_id":"abc","payload":{"sql":"select 1"}}"#;
    assert_eq!(s, expected);
    assert_eq!(serde_json::from_str::<DumpEvent>(&s).unwrap(), e);
}

#[test]
fn dump_ext_status_byte_shape() {
    let x = DumpExtStatus {
        version: PhpVersion::new(8, 3),
        present: true,
    };
    let s = serde_json::to_string(&x).unwrap();
    assert_eq!(s, r#"{"version":"8.3","present":true}"#);
    assert_eq!(serde_json::from_str::<DumpExtStatus>(&s).unwrap(), x);
}

#[test]
fn response_dumps_byte_shape() {
    let r = Response::Dumps {
        events: vec![DumpEvent {
            id: 1,
            category: DumpCategory::Dump,
            ts_ms: 1_718_360_452_123,
            site: "blog.test".into(),
            request_id: "abc".into(),
            payload: serde_json::json!({ "value_text": "hi" }),
        }],
        removed_ids: vec![3],
        counts: DumpCounts {
            dumps: 1,
            ..DumpCounts::default()
        },
        latest_id: 1,
        min_live_id: 1,
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected = r#"{"type":"dumps","events":[{"id":1,"category":"dump","ts_ms":1718360452123,"site":"blog.test","request_id":"abc","payload":{"value_text":"hi"}}],"removed_ids":[3],"counts":{"dumps":1,"queries":0,"jobs":0,"views":0,"requests":0,"logs":0,"cache":0,"http":0},"latest_id":1,"min_live_id":1}"#;
    assert_eq!(s, expected);
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

// ---------- Mail ----------

#[test]
fn request_list_mails_byte_shape() {
    let s = serde_json::to_string(&Request::ListMails).unwrap();
    assert_eq!(s, r#"{"type":"list_mails"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&s).unwrap(),
        Request::ListMails
    );
}

#[test]
fn request_get_mail_byte_shape() {
    let r = Request::GetMail {
        id: "000001".into(),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"get_mail","id":"000001"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_clear_mails_byte_shape() {
    let s = serde_json::to_string(&Request::ClearMails).unwrap();
    assert_eq!(s, r#"{"type":"clear_mails"}"#);
    assert_eq!(
        serde_json::from_str::<Request>(&s).unwrap(),
        Request::ClearMails
    );
}

#[test]
fn request_delete_mails_byte_shape() {
    let r = Request::DeleteMails {
        ids: vec!["000001".into(), "000002".into()],
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"delete_mails","ids":["000001","000002"]}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_set_mail_port_byte_shape() {
    let r = Request::SetMailPort { port: 2525 };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"set_mail_port","port":2525}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_set_mail_enabled_byte_shape() {
    let r = Request::SetMailEnabled { enabled: true };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"set_mail_enabled","enabled":true}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn response_mails_byte_shape() {
    let r = Response::Mails {
        mails: vec![MailSummary {
            id: "000001".into(),
            from: "Example <hello@example.com>".into(),
            to: vec!["test@test.com".into()],
            subject: "Hi".into(),
            date_epoch: 1_700_000_000,
        }],
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected = r#"{"type":"mails","mails":[{"id":"000001","from":"Example <hello@example.com>","to":["test@test.com"],"subject":"Hi","date_epoch":1700000000}]}"#;
    assert_eq!(s, expected);
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn response_dumps_status_byte_shape() {
    // Pin the full "every key present" features contract (see DumpsStatus doc),
    // not just one key, so wire drift in the map shape fails this test.
    let mut features = BTreeMap::new();
    features.insert("dumps".to_string(), true);
    features.insert("queries".to_string(), false);
    features.insert("jobs".to_string(), true);
    features.insert("views".to_string(), true);
    features.insert("requests".to_string(), true);
    features.insert("logs".to_string(), true);
    features.insert("cache".to_string(), true);
    features.insert("http".to_string(), true);
    let r = Response::DumpsStatus {
        enabled: true,
        port: 2304,
        running: true,
        persist: false,
        extensions: vec![DumpExtStatus {
            version: PhpVersion::new(8, 3),
            present: false,
        }],
        counts: DumpCounts::default(),
        features,
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected = r#"{"type":"dumps_status","enabled":true,"port":2304,"running":true,"persist":false,"extensions":[{"version":"8.3","present":false}],"counts":{"dumps":0,"queries":0,"jobs":0,"views":0,"requests":0,"logs":0,"cache":0,"http":0},"features":{"cache":true,"dumps":true,"http":true,"jobs":true,"logs":true,"queries":false,"requests":true,"views":true}}"#;
    assert_eq!(s, expected);
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn response_mail_byte_shape() {
    let r = Response::Mail {
        mail: Box::new(MailDetail {
            id: "000001".into(),
            from: "Example <hello@example.com>".into(),
            to: vec!["test@test.com".into()],
            subject: "Hi".into(),
            date_epoch: 1_700_000_000,
            headers: vec![MailHeader {
                name: "Subject".into(),
                value: "Hi".into(),
            }],
            html_body: Some("<p>Hi</p>".into()),
            text_body: None,
        }),
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected = r#"{"type":"mail","mail":{"id":"000001","from":"Example <hello@example.com>","to":["test@test.com"],"subject":"Hi","date_epoch":1700000000,"headers":[{"name":"Subject","value":"Hi"}],"html_body":"<p>Hi</p>","text_body":null}}"#;
    assert_eq!(s, expected);
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn status_mail_appears_only_when_some() {
    // `None` → omitted; `Some` → appended after `services`.
    let mut report = sample_status_report();
    let s = serde_json::to_string(&report).unwrap();
    assert!(!s.contains("mail"), "empty mail must be omitted: {s}");

    report.mail = Some(MailStatus {
        enabled: true,
        port: 2525,
        listening: true,
        count: 3,
    });
    let s = serde_json::to_string(&report).unwrap();
    assert!(
        s.contains(r#""mail":{"enabled":true,"port":2525,"listening":true,"count":3}"#),
        "{s}"
    );
    let back: StatusReport = serde_json::from_str(&s).unwrap();
    assert_eq!(back, report);
}

// ---------- Tools ----------

#[test]
fn request_list_tools_byte_shape() {
    let r = Request::ListTools;
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"list_tools"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_install_tool_byte_shape() {
    let r = Request::InstallTool {
        tool: "node".into(),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"install_tool","tool":"node"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_uninstall_tool_byte_shape() {
    let r = Request::UninstallTool { tool: "bun".into() };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"uninstall_tool","tool":"bun"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn response_tools_byte_shape() {
    let r = Response::Tools {
        tools: vec![ToolStatus {
            id: "node".into(),
            display_name: "Node.js".into(),
            installed: true,
            version: Some("v24.17.0".into()),
            binaries: vec!["node".into(), "npm".into(), "npx".into()],
        }],
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected = r#"{"type":"tools","tools":[{"id":"node","display_name":"Node.js","installed":true,"version":"v24.17.0","binaries":["node","npm","npx"]}]}"#;
    assert_eq!(s, expected);
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

// ---------- CreateSite / job model ----------

#[test]
fn request_create_site_byte_shape() {
    use yerd_ipc::{
        AuthProvider, CreateSiteSpec, Database, Framework, JsRuntime, LaravelOptions, StarterKit,
        Testing,
    };
    let r = Request::CreateSite {
        spec: CreateSiteSpec {
            name: "blog".into(),
            parent_dir: PathBuf::from("/srv"),
            php: PhpVersion::new(8, 4),
            secure: true,
            framework: Framework::Laravel {
                options: LaravelOptions {
                    starter_kit: StarterKit::React,
                    auth: AuthProvider::Laravel,
                    livewire_class_components: false,
                    teams: false,
                    testing: Testing::Pest,
                    database: Database::Sqlite,
                    js: JsRuntime::Npm,
                    git: true,
                    boost: false,
                },
            },
        },
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected = r#"{"type":"create_site","spec":{"name":"blog","parent_dir":"/srv","php":"8.4","secure":true,"framework":{"framework":"laravel","options":{"starter_kit":"react","auth":"laravel","livewire_class_components":false,"teams":false,"testing":"pest","database":"sqlite","js":"npm","git":true,"boost":false}}}}"#;
    assert_eq!(s, expected);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_create_site_community_kit_byte_shape() {
    use yerd_ipc::StarterKit;
    let s = serde_json::to_string(&StarterKit::Community("acme/kit".into())).unwrap();
    assert_eq!(s, r#"{"community":"acme/kit"}"#);
    assert_eq!(
        serde_json::from_str::<StarterKit>(&s).unwrap(),
        StarterKit::Community("acme/kit".into())
    );
}

#[test]
fn request_job_status_byte_shape() {
    let r = Request::JobStatus {
        job_id: "j1".into(),
        cursor: 7,
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"job_status","job_id":"j1","cursor":7}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn request_job_cancel_byte_shape() {
    let r = Request::JobCancel {
        job_id: "j1".into(),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"job_cancel","job_id":"j1"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn response_job_started_byte_shape() {
    let r = Response::JobStarted {
        job_id: "j1".into(),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"job_started","job_id":"j1"}"#);
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn response_job_progress_byte_shape() {
    use yerd_ipc::JobState;
    let r = Response::JobProgress {
        state: JobState::Running,
        phase: "Scaffolding".into(),
        log: vec!["line one".into()],
        next_cursor: 1,
        error: None,
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected = r#"{"type":"job_progress","state":"running","phase":"Scaffolding","log":["line one"],"next_cursor":1,"error":null}"#;
    assert_eq!(s, expected);
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn request_install_tool_streamed_byte_shape() {
    let r = Request::InstallToolStreamed {
        tool: "laravel".into(),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"install_tool_streamed","tool":"laravel"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

// ---------- Self-update (Channel / CheckUpdate / SetUpdateChannel / UpdateStatus) ----------

#[test]
fn channel_each_variant_byte_shape() {
    assert_eq!(
        serde_json::to_string(&Channel::Stable).unwrap(),
        r#""stable""#
    );
    assert_eq!(serde_json::to_string(&Channel::Edge).unwrap(), r#""edge""#);
    assert_eq!(
        serde_json::from_str::<Channel>(r#""stable""#).unwrap(),
        Channel::Stable
    );
    assert_eq!(
        serde_json::from_str::<Channel>(r#""edge""#).unwrap(),
        Channel::Edge
    );
}

#[test]
fn update_source_each_variant_byte_shape() {
    assert_eq!(
        serde_json::to_string(&UpdateSource::Live).unwrap(),
        r#""live""#
    );
    assert_eq!(
        serde_json::to_string(&UpdateSource::Cached).unwrap(),
        r#""cached""#
    );
    assert_eq!(
        serde_json::from_str::<UpdateSource>(r#""cached""#).unwrap(),
        UpdateSource::Cached
    );
}

#[test]
fn request_check_update_byte_shape() {
    let r = Request::CheckUpdate {
        channel: Some(Channel::Edge),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"check_update","channel":"edge"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);

    // `None` channel (use the configured default) serialises as null.
    let none = Request::CheckUpdate { channel: None };
    let s = serde_json::to_string(&none).unwrap();
    assert_eq!(s, r#"{"type":"check_update","channel":null}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), none);
}

#[test]
fn request_set_update_channel_byte_shape() {
    let r = Request::SetUpdateChannel {
        channel: Channel::Stable,
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"set_update_channel","channel":"stable"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn staged_artifact_each_variant_byte_shape() {
    assert_eq!(
        serde_json::to_string(&StagedArtifact::AppTarGz).unwrap(),
        r#""app_tar_gz""#
    );
    assert_eq!(
        serde_json::to_string(&StagedArtifact::Deb).unwrap(),
        r#""deb""#
    );
    assert_eq!(
        serde_json::from_str::<StagedArtifact>(r#""deb""#).unwrap(),
        StagedArtifact::Deb
    );
}

#[test]
fn request_stage_update_byte_shape() {
    let r = Request::StageUpdate {
        channel: Some(Channel::Stable),
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"type":"stage_update","channel":"stable"}"#);
    assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
}

#[test]
fn response_staged_byte_shape() {
    let r = Response::Staged {
        path: "/x/Yerd.app.tar.gz".into(),
        version: "2.0.5".into(),
        kind: StagedArtifact::AppTarGz,
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected =
        r#"{"type":"staged","path":"/x/Yerd.app.tar.gz","version":"2.0.5","kind":"app_tar_gz"}"#;
    assert_eq!(s, expected);
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}

#[test]
fn response_update_status_byte_shape() {
    let r = Response::UpdateStatus {
        current: "2.0.2-rc.3".into(),
        latest_stable: Some("2.0.1".into()),
        latest_edge: Some("2.0.2-rc.3".into()),
        channel: Channel::Stable,
        available: false,
        target: None,
        ahead_of_stable: true,
        source: UpdateSource::Live,
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected = r#"{"type":"update_status","current":"2.0.2-rc.3","latest_stable":"2.0.1","latest_edge":"2.0.2-rc.3","channel":"stable","available":false,"target":null,"ahead_of_stable":true,"source":"live"}"#;
    assert_eq!(s, expected);
    assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
}
