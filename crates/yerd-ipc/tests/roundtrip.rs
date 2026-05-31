//! `encode_message` ∘ `decode_message` round-trips, plus negative
//! tests pinning the "fail-closed on unknown tag" and "accept unknown
//! envelope fields / reject unknown Site fields" policies.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use yerd_ipc::{
    decode_message, encode_message,
    types::{PhpVersion, Site},
    CaStatus, Diagnosis, DiagnosisCode, ErrorCode, FixReport, FixResult, IpcError, PhpPoolStatus,
    PoolRunState, PortStatus, Request, Response, Severity, SiteCounts, StatusReport,
};

fn assert_request_roundtrips(r: Request) {
    let bytes = encode_message(&r).unwrap();
    let back: Request = decode_message(&bytes).unwrap();
    assert_eq!(back, r);
}

fn assert_response_roundtrips(r: Response) {
    let bytes = encode_message(&r).unwrap();
    let back: Response = decode_message(&bytes).unwrap();
    assert_eq!(back, r);
}

#[test]
fn encode_then_decode_request_roundtrip() {
    assert_request_roundtrips(Request::Ping);
    assert_request_roundtrips(Request::ListSites);
    assert_request_roundtrips(Request::Park {
        path: PathBuf::from("/srv/foo"),
    });
    assert_request_roundtrips(Request::Link {
        name: "foo".into(),
        path: PathBuf::from("/srv/foo"),
    });
    assert_request_roundtrips(Request::Unlink { name: "foo".into() });
    assert_request_roundtrips(Request::ListParked);
    assert_request_roundtrips(Request::Unpark {
        path: "/srv/sites".into(),
    });
    assert_request_roundtrips(Request::SetPhp {
        name: "foo".into(),
        version: PhpVersion::new(8, 3),
    });
    assert_request_roundtrips(Request::SetSecure {
        name: "foo".into(),
        secure: true,
    });
    assert_request_roundtrips(Request::DaemonInfo);
    assert_request_roundtrips(Request::InstallPhp {
        version: PhpVersion::new(8, 5),
    });
    assert_request_roundtrips(Request::SetDefaultPhp {
        version: PhpVersion::new(8, 4),
    });
    assert_request_roundtrips(Request::ListPhp);
    assert_request_roundtrips(Request::UpdatePhp {
        version: Some(PhpVersion::new(8, 5)),
    });
    assert_request_roundtrips(Request::UpdatePhp { version: None });
    assert_request_roundtrips(Request::CheckPhpUpdates);
    assert_request_roundtrips(Request::Status);
    assert_request_roundtrips(Request::Diagnose);
    assert_request_roundtrips(Request::DoctorFix);
}

#[test]
#[allow(clippy::too_many_lines)] // one roundtrip assertion per variant — naturally long
fn encode_then_decode_response_roundtrip() {
    assert_response_roundtrips(Response::Pong);
    assert_response_roundtrips(Response::Ok);
    assert_response_roundtrips(Response::Info {
        dns_addr: "127.0.0.1:1053".parse().unwrap(),
        tld: "test".into(),
        ca_path: PathBuf::from("/x/ca.cert.pem"),
        ca_fingerprint: "ab".repeat(32),
        http_port: 8080,
        https_port: 8443,
    });
    assert_response_roundtrips(Response::PhpVersions {
        installed: vec![PhpVersion::new(8, 3), PhpVersion::new(8, 5)],
        default: PhpVersion::new(8, 5),
        updates: vec![],
        settings: BTreeMap::new(),
    });
    assert_response_roundtrips(Response::PhpVersions {
        installed: vec![PhpVersion::new(8, 5)],
        default: PhpVersion::new(8, 5),
        updates: vec![yerd_ipc::PhpUpdate {
            version: PhpVersion::new(8, 5),
            installed: "8.5.6".into(),
            latest: "8.5.7".into(),
        }],
        settings: BTreeMap::from([("memory_limit".to_string(), "512M".to_string())]),
    });
    assert_response_roundtrips(Response::Parked { paths: vec![] });
    assert_response_roundtrips(Response::Parked {
        paths: vec!["/a".into(), "/b".into()],
    });
    assert_response_roundtrips(Response::Sites { sites: vec![] });
    let site = Site::parked("foo", "/srv/foo", PhpVersion::new(8, 3)).unwrap();
    assert_response_roundtrips(Response::Sites {
        sites: vec![site.clone()],
    });
    for code in [
        ErrorCode::NotFound,
        ErrorCode::AlreadyExists,
        ErrorCode::InvalidPath,
        ErrorCode::Internal,
    ] {
        assert_response_roundtrips(Response::Error {
            code,
            message: "x".into(),
        });
    }
    assert_response_roundtrips(Response::Status {
        report: Box::new(StatusReport {
            daemon_pid: 4242,
            uptime_secs: 7,
            daemon_rss_bytes: Some(2048),
            tld: "test".into(),
            http: PortStatus {
                requested: 80,
                bound: 80,
                fell_back: false,
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
                trusted_system: None,
            },
            resolver_installed: Some(false),
            port_redirect: Some(true),
            default_php: PhpVersion::new(8, 5),
            php: vec![PhpPoolStatus {
                version: PhpVersion::new(8, 5),
                installed_patch: Some("8.5.6".into()),
                state: PoolRunState::Stopped,
                pid: None,
                listen: None,
                rss_bytes: None,
                update_available: Some("8.5.7".into()),
            }],
            sites: SiteCounts {
                parked: 1,
                linked: 0,
                secured: 0,
            },
            load_avg: None,
            daemon_version: "2.0.1".into(),
        }),
    });
    assert_response_roundtrips(Response::Diagnoses {
        items: vec![Diagnosis {
            code: DiagnosisCode::AllGood,
            severity: Severity::Ok,
            title: "all good".into(),
            detail: String::new(),
            remedy: None,
        }],
    });
    assert_response_roundtrips(Response::DoctorFix {
        report: FixReport {
            performed: vec![FixResult {
                code: DiagnosisCode::FpmPoolFailed,
                ok: true,
                message: "restarted".into(),
            }],
            manual: vec![Diagnosis {
                code: DiagnosisCode::ResolverNotInstalled,
                severity: Severity::Warn,
                title: "resolver".into(),
                detail: String::new(),
                remedy: Some("sudo yerd elevate resolver".into()),
            }],
        },
    });
}

#[test]
fn decode_rejects_unknown_type_tag() {
    let bytes = br#"{"type":"this_is_not_a_known_variant"}"#;
    let err = decode_message::<Request>(bytes).unwrap_err();
    assert!(matches!(err, IpcError::Decode(_)), "got {err:?}");
}

#[test]
fn decode_rejects_missing_required_field() {
    // `Link` requires both `name` and `path`; omit `path`.
    let bytes = br#"{"type":"link","name":"foo"}"#;
    let err = decode_message::<Request>(bytes).unwrap_err();
    assert!(matches!(err, IpcError::Decode(_)), "got {err:?}");
}

#[test]
fn decode_accepts_unknown_envelope_field() {
    // The envelope tolerates additive fields so newer daemons can
    // extend requests/responses without breaking older clients.
    let bytes = br#"{"type":"ping","__extra":42}"#;
    let r: Request = decode_message(bytes).unwrap();
    assert_eq!(r, Request::Ping);
}

#[test]
fn decode_rejects_unknown_field_inside_site() {
    // `yerd-core::Site`'s Deserialize impl is strict: unknown fields
    // on the *inner* Site payload are rejected. This is the
    // intentional asymmetry — envelope-permissive, payload-strict.
    let bytes = br#"{"type":"sites","sites":[{"name":"foo","document_root":"/srv/foo","php":"8.3","secure":false,"kind":"parked","surprise":1}]}"#;
    let err = decode_message::<Response>(bytes).unwrap_err();
    assert!(matches!(err, IpcError::Decode(_)), "got {err:?}");
}

#[test]
fn decode_rejects_unknown_error_code() {
    // Fail-closed on unknown ErrorCode (no #[serde(other)] Unknown).
    let bytes = br#"{"type":"error","code":"rate_limited","message":"x"}"#;
    let err = decode_message::<Response>(bytes).unwrap_err();
    assert!(matches!(err, IpcError::Decode(_)), "got {err:?}");
}
