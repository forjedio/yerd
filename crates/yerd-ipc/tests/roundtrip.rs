//! `encode_message` ∘ `decode_message` round-trips, plus negative
//! tests pinning the "fail-closed on unknown tag" and "accept unknown
//! envelope fields / reject unknown Site fields" policies.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;

use yerd_ipc::{
    decode_message, encode_message,
    types::{PhpVersion, Site},
    ErrorCode, IpcError, Request, Response,
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
    assert_request_roundtrips(Request::SetPhp {
        name: "foo".into(),
        version: PhpVersion::new(8, 3),
    });
    assert_request_roundtrips(Request::SetSecure {
        name: "foo".into(),
        secure: true,
    });
    assert_request_roundtrips(Request::DaemonInfo);
}

#[test]
fn encode_then_decode_response_roundtrip() {
    assert_response_roundtrips(Response::Pong);
    assert_response_roundtrips(Response::Ok);
    assert_response_roundtrips(Response::Info {
        dns_addr: "127.0.0.1:1053".parse().unwrap(),
        tld: "test".into(),
        ca_path: PathBuf::from("/x/ca.cert.pem"),
        ca_fingerprint: "ab".repeat(32),
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
