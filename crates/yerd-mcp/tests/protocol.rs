//! Protocol-level behaviour of the [`Server`] state machine: the handshake, the
//! notification/request split, error mapping, and the availability gate.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use serde_json::{json, Value};
use yerd_mcp::{Availability, Outgoing, Server, LATEST_PROTOCOL_VERSION};

fn server(availability: Availability) -> Server {
    Server::new(availability, "9.9.9")
}

/// An initialized server, which is the state most methods require.
fn ready(availability: Availability) -> Server {
    let mut s = server(availability);
    let _ = s.handle_line(&initialize_line(Some(LATEST_PROTOCOL_VERSION)));
    s
}

fn initialize_line(version: Option<&str>) -> String {
    let params = match version {
        Some(v) => {
            json!({ "protocolVersion": v, "capabilities": {}, "clientInfo": { "name": "probe", "version": "0" } })
        }
        None => json!({ "capabilities": {} }),
    };
    json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": params }).to_string()
}

fn call_line(id: i64, name: &str, args: Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": name, "arguments": args },
    })
    .to_string()
}

fn reply(out: Outgoing) -> Value {
    match out {
        Outgoing::Reply(s) => serde_json::from_str(&s).expect("reply is valid JSON"),
        other => panic!("expected a reply, got {other:?}"),
    }
}

fn error_code(out: Outgoing) -> i64 {
    reply(out)
        .pointer("/error/code")
        .and_then(Value::as_i64)
        .expect("error code present")
}

#[test]
fn initialize_echoes_every_supported_version() {
    for version in yerd_mcp::SUPPORTED_PROTOCOL_VERSIONS {
        let mut s = server(Availability::Enabled);
        let v = reply(s.handle_line(&initialize_line(Some(version))));
        assert_eq!(
            v.pointer("/result/protocolVersion").and_then(Value::as_str),
            Some(*version),
            "should echo the client's supported version"
        );
    }
}

#[test]
fn initialize_offers_latest_for_unknown_or_absent_version() {
    for line in [initialize_line(Some("1999-01-01")), initialize_line(None)] {
        let mut s = server(Availability::Enabled);
        let v = reply(s.handle_line(&line));
        assert_eq!(
            v.pointer("/result/protocolVersion").and_then(Value::as_str),
            Some(LATEST_PROTOCOL_VERSION)
        );
    }
}

#[test]
fn initialize_result_shape_is_camel_case_and_declares_tools() {
    let mut s = server(Availability::Enabled);
    let v = reply(s.handle_line(&initialize_line(Some(LATEST_PROTOCOL_VERSION))));
    assert_eq!(v["jsonrpc"], json!("2.0"));
    assert_eq!(v["id"], json!(1));
    assert_eq!(v.pointer("/result/capabilities/tools"), Some(&json!({})));
    assert_eq!(
        v.pointer("/result/serverInfo/name").and_then(Value::as_str),
        Some("yerd")
    );
    assert_eq!(
        v.pointer("/result/serverInfo/version")
            .and_then(Value::as_str),
        Some("9.9.9")
    );
    assert!(v.pointer("/result/instructions").is_some());
}

#[test]
fn initialize_instructions_differ_per_availability() {
    let mut texts = vec![];
    for availability in [
        Availability::Enabled,
        Availability::Disabled,
        Availability::Unknown,
    ] {
        let mut s = server(availability);
        let v = reply(s.handle_line(&initialize_line(Some(LATEST_PROTOCOL_VERSION))));
        let text = v
            .pointer("/result/instructions")
            .and_then(Value::as_str)
            .expect("instructions present")
            .to_owned();
        texts.push(text);
    }
    let enabled = texts.first().expect("enabled text");
    let disabled = texts.get(1).expect("disabled text");
    let unknown = texts.get(2).expect("unknown text");
    assert!(enabled.contains("job_status"), "enabled explains polling");
    assert!(disabled.contains("disabled"), "disabled says so");
    assert!(
        unknown.contains("not reachable") || unknown.contains("not running"),
        "unknown blames the daemon, not the toggle: {unknown}"
    );
    assert_ne!(enabled, disabled);
    assert_ne!(disabled, unknown);
}

#[test]
fn double_initialize_is_answered_twice() {
    let mut s = server(Availability::Enabled);
    let first = reply(s.handle_line(&initialize_line(Some(LATEST_PROTOCOL_VERSION))));
    let second = reply(s.handle_line(&initialize_line(Some(LATEST_PROTOCOL_VERSION))));
    assert!(first.pointer("/result").is_some());
    assert!(second.pointer("/result").is_some());
}

#[test]
fn ping_is_answerable_before_and_after_initialize() {
    let line = json!({ "jsonrpc": "2.0", "id": 7, "method": "ping" }).to_string();

    let mut before = server(Availability::Enabled);
    let v = reply(before.handle_line(&line));
    assert_eq!(v["result"], json!({}), "pre-initialize ping is allowed");
    assert_eq!(v["id"], json!(7));

    let mut after = ready(Availability::Enabled);
    assert_eq!(reply(after.handle_line(&line))["result"], json!({}));
}

#[test]
fn tools_before_initialize_are_rejected() {
    for method in ["tools/list", "tools/call"] {
        let mut s = server(Availability::Enabled);
        let line = json!({ "jsonrpc": "2.0", "id": 2, "method": method, "params": { "name": "list_sites" } })
            .to_string();
        assert_eq!(error_code(s.handle_line(&line)), -32000, "{method}");
    }
}

/// A client may call `tools/list` as soon as it has the initialize *result*.
/// Readiness keys on having answered `initialize`, not on the client's
/// `notifications/initialized`, or a lax client is locked out for good.
#[test]
fn tools_list_works_before_the_initialized_notification() {
    let mut s = server(Availability::Enabled);
    let _ = s.handle_line(&initialize_line(Some(LATEST_PROTOCOL_VERSION)));
    let line = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }).to_string();
    let v = reply(s.handle_line(&line));
    assert!(
        v.pointer("/result/tools")
            .and_then(Value::as_array)
            .is_some_and(|t| !t.is_empty()),
        "tools listed without notifications/initialized"
    );
}

#[test]
fn notifications_are_never_answered() {
    let mut s = ready(Availability::Enabled);
    for line in [
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
        json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 1 } })
            .to_string(),
        json!({ "jsonrpc": "2.0", "method": "notifications/something_new" }).to_string(),
    ] {
        assert_eq!(s.handle_line(&line), Outgoing::None, "for {line}");
    }
}

/// This server issues no requests, so it should never receive a response. If one
/// arrives it is ignored: replying to a reply is a protocol violation, and the
/// `id` belongs to the *sender's* numbering, not ours.
#[test]
fn stray_response_is_ignored() {
    let mut s = ready(Availability::Enabled);
    for line in [
        json!({ "jsonrpc": "2.0", "id": 9, "result": { "anything": true } }).to_string(),
        json!({ "jsonrpc": "2.0", "id": 9, "error": { "code": -1, "message": "x" } }).to_string(),
    ] {
        assert_eq!(s.handle_line(&line), Outgoing::None, "for {line}");
    }
}

/// A message that carries an `id` but no usable `method` is a malformed request,
/// not a notification. The distinction matters: treating it as a notification
/// answers nothing, and the client blocks forever on an id it will never see
/// again.
#[test]
fn a_waiting_client_is_never_left_without_a_reply() {
    let mut s = ready(Availability::Enabled);
    for line in [
        json!({ "jsonrpc": "2.0", "id": 10, "method": 123 }).to_string(),
        json!({ "jsonrpc": "2.0", "id": 11, "method": null }).to_string(),
        json!({ "jsonrpc": "2.0", "id": 12, "method": ["ping"] }).to_string(),
        json!({ "jsonrpc": "2.0", "id": 13 }).to_string(),
    ] {
        let out = s.handle_line(&line);
        assert_eq!(error_code(out.clone()), -32600, "for {line}");
        let v = reply(out);
        assert!(
            !v["id"].is_null(),
            "the reply must carry the caller's id so it can retire the call: {v}"
        );
    }
}

/// The same shapes without an `id`: nobody is waiting, so silence is correct.
#[test]
fn malformed_notifications_are_still_not_answered() {
    let mut s = ready(Availability::Enabled);
    for line in [
        json!({ "jsonrpc": "2.0", "method": 123 }).to_string(),
        json!({ "jsonrpc": "2.0" }).to_string(),
    ] {
        assert_eq!(s.handle_line(&line), Outgoing::None, "for {line}");
    }
}

#[test]
fn unknown_request_method_is_method_not_found() {
    let mut s = ready(Availability::Enabled);
    let line = json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/list" }).to_string();
    assert_eq!(error_code(s.handle_line(&line)), -32601);
}

#[test]
fn malformed_json_is_a_parse_error_with_null_id() {
    let mut s = ready(Availability::Enabled);
    let v = reply(s.handle_line("{not json"));
    assert_eq!(
        v.pointer("/error/code").and_then(Value::as_i64),
        Some(-32700)
    );
    assert_eq!(v["id"], Value::Null);
}

#[test]
fn batch_array_is_an_invalid_request() {
    let mut s = ready(Availability::Enabled);
    let line = json!([{ "jsonrpc": "2.0", "id": 1, "method": "ping" }]).to_string();
    assert_eq!(error_code(s.handle_line(&line)), -32600);
}

#[test]
fn non_object_json_is_an_invalid_request() {
    let mut s = ready(Availability::Enabled);
    assert_eq!(error_code(s.handle_line("42")), -32600);
}

#[test]
fn unknown_tool_is_invalid_params() {
    let mut s = ready(Availability::Enabled);
    assert_eq!(
        error_code(s.handle_line(&call_line(5, "rm_rf_everything", json!({})))),
        -32602
    );
}

#[test]
fn argument_validation_errors_are_invalid_params() {
    let cases = [
        ("link_site", json!({ "name": "foo" })),
        ("link_site", json!({ "name": 42, "path": "/srv/foo" })),
        ("set_site_php", json!({ "name": "foo", "version": "nope" })),
        ("set_site_secure", json!({ "name": "foo" })),
        (
            "create_site",
            json!({ "name": "foo", "parent_dir": "/srv", "php": "8.4", "starter_kit": "angular" }),
        ),
        ("list_dumps", json!({ "since_id": -1 })),
        ("job_status", json!({ "cursor": 0 })),
    ];
    let mut s = ready(Availability::Enabled);
    for (tool, args) in cases {
        assert_eq!(
            error_code(s.handle_line(&call_line(6, tool, args.clone()))),
            -32602,
            "{tool} with {args}"
        );
    }
}

#[test]
fn enabled_tool_call_goes_straight_to_the_daemon() {
    let mut s = ready(Availability::Enabled);
    match s.handle_line(&call_line(8, "list_sites", json!({}))) {
        Outgoing::CallDaemon(call) => {
            assert_eq!(call.request, yerd_ipc::Request::ListSites);
            assert_eq!(call.tool(), "list_sites");
        }
        other => panic!("expected CallDaemon, got {other:?}"),
    }
}

#[test]
fn gated_tool_call_is_policy_blocked_with_guidance_and_a_retry() {
    for (availability, expected) in [
        (Availability::Disabled, "disabled"),
        (Availability::Unknown, "could not be reached"),
    ] {
        let mut s = ready(availability);
        match s.handle_line(&call_line(9, "list_sites", json!({}))) {
            Outgoing::PolicyBlocked(call) => {
                assert_eq!(
                    call.request,
                    yerd_ipc::Request::ListSites,
                    "the parsed call is carried for re-dispatch"
                );
                let v: Value = serde_json::from_str(&s.gate_reply(&call)).expect("valid JSON");
                assert_eq!(v["id"], json!(9), "guidance keeps the call's id");
                assert_eq!(v.pointer("/result/isError"), Some(&json!(true)));
                let text = v
                    .pointer("/result/content/0/text")
                    .and_then(Value::as_str)
                    .expect("text content");
                assert!(text.contains(expected), "{availability:?}: {text}");
            }
            other => panic!("expected PolicyBlocked for {availability:?}, got {other:?}"),
        }
    }
}

/// A malformed call is reported as malformed even while gated. Otherwise the
/// agent is told to fix the wrong thing, and retries straight into the same
/// error once the user enables the feature for it.
#[test]
fn validation_beats_the_gate() {
    let mut s = ready(Availability::Disabled);
    assert_eq!(
        error_code(s.handle_line(&call_line(10, "no_such_tool", json!({})))),
        -32602
    );
    assert_eq!(
        error_code(s.handle_line(&call_line(11, "link_site", json!({ "name": "foo" })))),
        -32602
    );
}

#[test]
fn set_availability_unblocks_a_previously_gated_session() {
    let mut s = ready(Availability::Disabled);
    assert!(matches!(
        s.handle_line(&call_line(12, "list_sites", json!({}))),
        Outgoing::PolicyBlocked(_)
    ));
    s.set_availability(Availability::Enabled);
    assert!(matches!(
        s.handle_line(&call_line(13, "list_sites", json!({}))),
        Outgoing::CallDaemon(_)
    ));
}

/// A session that starts with no daemon, then reaches one and finds the toggle
/// off, must stop blaming the daemon - otherwise the user goes looking for a
/// process that is running fine.
#[test]
fn gate_reply_tracks_the_current_reason_not_the_startup_one() {
    let mut s = ready(Availability::Unknown);
    let Outgoing::PolicyBlocked(call) = s.handle_line(&call_line(14, "list_sites", json!({})))
    else {
        panic!("expected PolicyBlocked");
    };
    assert!(s.gate_reply(&call).contains("could not be reached"));

    s.set_availability(Availability::Disabled);
    let now = s.gate_reply(&call);
    assert!(now.contains("disabled"), "{now}");
    assert!(!now.contains("could not be reached"), "{now}");
}

#[test]
fn parse_error_reply_is_a_null_id_protocol_error() {
    let v: Value =
        serde_json::from_str(&yerd_mcp::parse_error_reply("line too long")).expect("valid JSON");
    assert_eq!(v["id"], Value::Null);
    assert_eq!(v.pointer("/error/code"), Some(&json!(-32700)));
    assert_eq!(v.pointer("/error/message"), Some(&json!("line too long")));
}

/// stdout framing is newline-delimited, so an embedded newline would split one
/// message into two and desynchronise the client for the rest of the session.
#[test]
fn every_reply_is_a_single_line() {
    let mut s = ready(Availability::Enabled);
    let lines = [
        initialize_line(Some(LATEST_PROTOCOL_VERSION)),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string(),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "nope" }).to_string(),
        "{bad".to_owned(),
    ];
    for line in lines {
        if let Outgoing::Reply(out) = s.handle_line(&line) {
            assert!(!out.contains('\n'), "embedded newline in: {out}");
        }
    }
}
