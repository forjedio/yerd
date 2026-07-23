//! Tool-result rendering: content shape, error mapping, status trimming, and
//! the job-polling hint.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;

use serde_json::{json, Value};
use yerd_core::PhpVersion;
use yerd_ipc::{ErrorCode, Response};
use yerd_mcp::{Availability, Outgoing, PendingCall, Server, LATEST_PROTOCOL_VERSION};

fn ready() -> Server {
    let mut s = Server::new(Availability::Enabled, "9.9.9");
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": LATEST_PROTOCOL_VERSION, "capabilities": {} },
    })
    .to_string();
    let _ = s.handle_line(&init);
    s
}

fn pending(tool: &str, args: Value) -> PendingCall {
    let line = json!({
        "jsonrpc": "2.0", "id": 77, "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    })
    .to_string();
    match ready().handle_line(&line) {
        Outgoing::CallDaemon(call) => call,
        other => panic!("expected CallDaemon, got {other:?}"),
    }
}

/// Complete a call and return the parsed JSON-RPC reply.
fn complete(tool: &str, args: Value, result: Result<Response, String>) -> Value {
    serde_json::from_str(&pending(tool, args).complete(result)).expect("valid JSON reply")
}

/// The single text content item of a tool result.
fn text(reply: &Value) -> &str {
    reply
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .expect("text content")
}

fn is_error(reply: &Value) -> bool {
    reply
        .pointer("/result/isError")
        .and_then(Value::as_bool)
        .expect("isError present")
}

fn sample_report() -> yerd_ipc::StatusReport {
    yerd_ipc::StatusReport {
        daemon_pid: 4242,
        uptime_secs: 7,
        daemon_rss_bytes: Some(2048),
        tld: "test".into(),
        http: yerd_ipc::PortStatus {
            requested: 80,
            bound: 8080,
            fell_back: true,
        },
        https: yerd_ipc::PortStatus {
            requested: 443,
            bound: 8443,
            fell_back: true,
        },
        dns_addr: "127.0.0.1:1053".parse().unwrap(),
        ca: yerd_ipc::CaStatus {
            path: PathBuf::from("/x/ca.cert.pem"),
            fingerprint: "ab".repeat(32),
            trusted_system: Some(true),
            php_trusts_ca: None,
            browser_trust: None,
        },
        resolver_installed: Some(true),
        port_redirect: None,
        foreign_web_listener: None,
        resolver_backup: None,
        default_php: PhpVersion::new(8, 4),
        php: vec![yerd_ipc::PhpPoolStatus {
            version: PhpVersion::new(8, 4),
            installed_patch: Some("8.4.1".into()),
            state: yerd_ipc::PoolRunState::Running,
            pid: Some(99),
            listen: Some("/run/fpm.sock".into()),
            rss_bytes: Some(1024),
            update_available: None,
        }],
        sites: yerd_ipc::SiteCounts {
            parked: 1,
            linked: 2,
            secured: 1,
        },
        load_avg: Some([100, 50, 25]),
        daemon_version: "2.0.3".into(),
        services: vec![yerd_ipc::ServiceStatus {
            service: "mysql".into(),
            display_name: "MySQL".into(),
            installed_versions: vec!["8.4".into()],
            selected_version: Some("8.4".into()),
            state: yerd_ipc::ServiceRunState::Running,
            pid: Some(7),
            listen: Some("127.0.0.1:3306".into()),
            port: 3306,
            enabled: true,
            supports_databases: true,
            type_id: "mysql".into(),
            site: None,
            error: None,
        }],
        mail: Some(yerd_ipc::MailStatus {
            enabled: true,
            port: 2525,
            listening: true,
            count: 3,
            unread: 2,
        }),
        web_unbound: None,
        dns_unbound: None,
        boot_id: Some(1),
        shared_sites: 0,
        symlink_protection: true,
        shadows: vec![],
        mcp_enabled: true,
        lan_enabled: false,
        lan_ip: None,
        lan_setup_bound: None,
        port_redirect_targets: Some(yerd_ipc::PortRedirectTargets {
            http: 8080,
            https: 8443,
        }),
        lan_redirect_targets: Some(yerd_ipc::PortRedirectTargets {
            http: 8080,
            https: 8443,
        }),
    }
}

#[test]
fn tool_results_carry_one_text_content_item() {
    let reply = complete(
        "list_sites",
        json!({}),
        Ok(Response::Sites { sites: vec![] }),
    );
    assert_eq!(reply["id"], json!(77), "the call's id is echoed");
    assert_eq!(
        reply.pointer("/result/content/0/type"),
        Some(&json!("text")),
        "content items are typed"
    );
    assert!(!is_error(&reply));
}

#[test]
fn ok_responses_render_as_their_wire_json() {
    let reply = complete(
        "park_directory",
        json!({ "path": "/srv" }),
        Ok(Response::Ok),
    );
    let payload: Value = serde_json::from_str(text(&reply)).expect("text is JSON");
    assert_eq!(payload, json!({ "type": "ok" }));
}

#[test]
fn daemon_errors_render_as_failed_tool_results() {
    let reply = complete(
        "set_site_php",
        json!({ "name": "nope", "version": "8.4" }),
        Ok(Response::Error {
            code: ErrorCode::SiteNotFound,
            message: "no such site `nope`".into(),
        }),
    );
    assert!(is_error(&reply), "a daemon error is a failed tool call");
    let body = text(&reply);
    assert!(
        body.contains("no such site"),
        "keeps the daemon's message: {body}"
    );
    assert!(body.contains("site_not_found"), "names the code: {body}");
    assert!(
        reply.pointer("/error").is_none(),
        "a failed operation is not a JSON-RPC error"
    );
}

#[test]
fn transport_failures_render_as_failed_tool_results() {
    let reply = complete(
        "list_sites",
        json!({}),
        Err("The Yerd daemon is not running.".to_owned()),
    );
    assert!(is_error(&reply));
    assert_eq!(text(&reply), "The Yerd daemon is not running.");
}

#[test]
fn job_started_carries_the_polling_hint() {
    let reply = complete(
        "install_php",
        json!({ "version": "8.4" }),
        Ok(Response::JobStarted {
            job_id: "job-1".into(),
        }),
    );
    assert!(!is_error(&reply));
    let payload: Value = serde_json::from_str(text(&reply)).expect("text is JSON");
    assert_eq!(payload["job_id"], json!("job-1"));
    let hint = payload["hint"].as_str().expect("hint present");
    assert!(
        hint.contains("job_status"),
        "names the tool to poll: {hint}"
    );
    assert!(hint.contains("next_cursor"), "explains the cursor: {hint}");
}

/// Jobs live in memory, so after a daemon restart the id an agent is holding is
/// simply gone. A bare "not found" would read as "the work failed", which is a
/// different thing to report to a user.
#[test]
fn unknown_job_explains_that_jobs_are_ephemeral() {
    let reply = complete(
        "job_status",
        json!({ "job_id": "job-1" }),
        Ok(Response::Error {
            code: ErrorCode::NotFound,
            message: "unknown job `job-1`".into(),
        }),
    );
    assert!(is_error(&reply));
    let body = text(&reply);
    assert!(body.contains("unknown job"), "{body}");
    assert!(
        body.contains("restarted"),
        "explains the likely cause: {body}"
    );
    assert!(
        body.contains("list_sites"),
        "suggests how to verify: {body}"
    );
}

/// The ephemeral-job hint is keyed on the *code* as well as the tool: a
/// `job_status` failure that is not `NotFound` is a real failure and must not be
/// explained away as an expired job.
#[test]
fn job_status_errors_other_than_not_found_keep_the_plain_rendering() {
    let reply = complete(
        "job_status",
        json!({ "job_id": "job-1" }),
        Ok(Response::Error {
            code: ErrorCode::Internal,
            message: "job runner exploded".into(),
        }),
    );
    assert!(is_error(&reply));
    let body = text(&reply);
    assert!(body.contains("job runner exploded"), "{body}");
    assert!(
        !body.contains("restarted") && !body.contains("expired"),
        "a real failure must not be dressed up as an expired job: {body}"
    );
}

#[test]
fn not_found_from_other_tools_keeps_the_plain_rendering() {
    let reply = complete(
        "get_mail",
        json!({ "id": "1" }),
        Ok(Response::Error {
            code: ErrorCode::NotFound,
            message: "no such mail".into(),
        }),
    );
    let body = text(&reply);
    assert!(body.contains("no such mail"));
    assert!(
        !body.contains("restarted"),
        "the job hint is job_status-only: {body}"
    );
}

#[test]
fn status_is_trimmed_to_what_an_agent_can_act_on() {
    let reply = complete(
        "status",
        json!({}),
        Ok(Response::Status {
            report: Box::new(sample_report()),
        }),
    );
    let s: Value = serde_json::from_str(text(&reply)).expect("text is JSON");

    assert_eq!(s["daemon_version"], json!("2.0.3"));
    assert_eq!(s["tld"], json!("test"));
    assert_eq!(
        s["http"],
        json!({ "requested": 80, "bound": 8080, "fell_back": true })
    );
    assert_eq!(s["dns_addr"], json!("127.0.0.1:1053"));
    assert_eq!(s["ca_trusted_system"], json!(true));
    assert_eq!(s["default_php"], json!("8.4"));
    assert_eq!(s["resolver_installed"], json!(true));
    assert_eq!(s["symlink_protection"], json!(true));
    assert_eq!(s["mcp_enabled"], json!(true));
    assert_eq!(
        s["sites"],
        json!({ "parked": 1, "linked": 2, "secured": 1 })
    );
    assert_eq!(
        s["php"],
        json!([{ "version": "8.4", "state": "running", "installed_patch": "8.4.1", "update_available": null }])
    );
    assert_eq!(
        s["services"],
        json!([{ "id": "mysql", "running": true, "port": 3306 }]),
        "services report derived id/running, not the raw wire struct"
    );
    assert_eq!(
        s["mail"],
        json!({ "enabled": true, "listening": true, "port": 2525, "unread": 2 })
    );

    for dropped in [
        "daemon_pid",
        "daemon_rss_bytes",
        "load_avg",
        "boot_id",
        "ca",
        "resolver_backup",
        "port_redirect",
        "port_redirect_targets",
        "lan_redirect_targets",
        "shared_sites",
        "lan_enabled",
        "lan_ip",
        "lan_setup_bound",
    ] {
        assert!(
            s.get(dropped).is_none(),
            "{dropped} is host detail an agent pays tokens for and cannot use"
        );
    }
}

#[test]
fn status_reports_degraded_listeners() {
    let mut report = sample_report();
    report.web_unbound = Some(yerd_ipc::UnboundWeb {
        http: 8080,
        https: 8443,
    });
    report.dns_unbound = Some(1053);
    let reply = complete(
        "status",
        json!({}),
        Ok(Response::Status {
            report: Box::new(report),
        }),
    );
    let s: Value = serde_json::from_str(text(&reply)).expect("text is JSON");
    assert_eq!(s["web_unbound"], json!({ "http": 8080, "https": 8443 }));
    assert_eq!(s["dns_unbound"], json!(1053));
}

#[test]
fn rendered_replies_are_single_line() {
    let mut report = sample_report();
    report.tld = "line\nbreak".into();
    let line = pending("status", json!({}).clone()).complete(Ok(Response::Status {
        report: Box::new(report),
    }));
    assert!(
        !line.contains('\n'),
        "newlines in payload data must stay escaped: {line}"
    );
}
