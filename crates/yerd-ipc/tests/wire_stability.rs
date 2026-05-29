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

use std::path::PathBuf;

use yerd_ipc::{
    types::{PhpVersion, Site},
    ErrorCode, Request, Response,
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
fn request_daemon_info_byte_shape() {
    let s = serde_json::to_string(&Request::DaemonInfo).unwrap();
    assert_eq!(s, r#"{"type":"daemon_info"}"#);
    let back: Request = serde_json::from_str(&s).unwrap();
    assert_eq!(back, Request::DaemonInfo);
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
fn response_info_byte_shape() {
    let r = Response::Info {
        dns_addr: "127.0.0.1:1053".parse().unwrap(),
        tld: "test".into(),
        ca_path: std::path::PathBuf::from("/home/u/.local/share/yerd/ca.cert.pem"),
        ca_fingerprint: "ab".repeat(32),
    };
    let s = serde_json::to_string(&r).unwrap();
    let expected = format!(
        r#"{{"type":"info","dns_addr":"127.0.0.1:1053","tld":"test","ca_path":"/home/u/.local/share/yerd/ca.cert.pem","ca_fingerprint":"{}"}}"#,
        "ab".repeat(32)
    );
    assert_eq!(s, expected);
    let back: Response = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
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

// ---------- ErrorCode (standalone) ----------

#[test]
fn error_code_each_variant_byte_shape() {
    let cases: &[(ErrorCode, &str)] = &[
        (ErrorCode::NotFound, r#""not_found""#),
        (ErrorCode::AlreadyExists, r#""already_exists""#),
        (ErrorCode::InvalidPath, r#""invalid_path""#),
        (ErrorCode::Internal, r#""internal""#),
    ];
    for (code, expected) in cases {
        let s = serde_json::to_string(code).unwrap();
        assert_eq!(&s, expected, "code = {code:?}");
        let back: ErrorCode = serde_json::from_str(&s).unwrap();
        assert_eq!(back, *code);
    }
}
