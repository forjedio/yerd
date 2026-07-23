//! Turning daemon answers into MCP tool results.
//!
//! A tool result is `{"content":[{"type":"text","text":…}],"isError":bool}`.
//! The text is JSON, so agents can parse it, and it keeps the daemon's own
//! `"type"` discriminator (the wire enum is internally tagged): every payload
//! therefore says what kind of answer it is, which is also why no separate
//! "unexpected response variant" check is needed here - a mismatch would be
//! visible in the rendered text.
//!
//! Two answers are reshaped rather than passed through. [`yerd_ipc::Response::Status`]
//! is trimmed, because the full report carries process/host detail an agent has
//! no use for and pays tokens for. [`yerd_ipc::Response::JobStarted`] gains the
//! polling hint, because a bare job id tells an agent nothing about what to do
//! next.

use serde::Serialize;
use serde_json::{json, Value};
use yerd_ipc::{ErrorCode, Response, ServiceRunState, StatusReport};

pub(crate) fn render(tool: &str, response: &Response) -> Value {
    match response {
        Response::Error { code, message } => render_error(tool, *code, message),
        Response::JobStarted { job_id } => text_result(&encode(&json!({
            "job_id": job_id,
            "hint": "Work started in the background. Poll the job_status tool with this job_id, \
                     passing the next_cursor from each poll, until state is succeeded, failed, or \
                     cancelled.",
        }))),
        Response::Status { report } => text_result(&encode(&trim_status(report))),
        other => text_result(&encode(&to_value(other))),
    }
}

/// A failed tool call: the operation was attempted (or deliberately not
/// attempted) and could not be completed. Distinct from a JSON-RPC error, which
/// would mean the *call* was malformed.
pub(crate) fn tool_error(text: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": true,
    })
}

fn text_result(text: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false,
    })
}

fn render_error(tool: &str, code: ErrorCode, message: &str) -> Value {
    if tool == "job_status" && code == ErrorCode::NotFound {
        return tool_error(&format!(
            "{message}. Jobs are held in memory, so this one has either already been pruned or was \
             lost when the daemon restarted. Check the outcome directly instead, e.g. with \
             list_sites or list_php."
        ));
    }
    let code = to_value(&code);
    let code = code.as_str().unwrap_or("error");
    tool_error(&format!("{message} (daemon error: {code})"))
}

/// Keep what an agent can act on; drop host/process detail (pid, RSS, load
/// average, boot id, CA path and fingerprint, resolver backup, port redirect and
/// its anchor target ports). This is an allowlist projection, so new host-only
/// `StatusReport` fields are excluded by default.
fn trim_status(report: &StatusReport) -> Value {
    let php: Vec<Value> = report
        .php
        .iter()
        .map(|p| {
            json!({
                "version": p.version.to_string(),
                "state": to_value(&p.state),
                "installed_patch": p.installed_patch,
                "update_available": p.update_available,
            })
        })
        .collect();
    let services: Vec<Value> = report
        .services
        .iter()
        .map(|s| {
            json!({
                "id": s.service,
                "running": s.state == ServiceRunState::Running,
                "port": s.port,
            })
        })
        .collect();
    json!({
        "daemon_version": report.daemon_version,
        "uptime_secs": report.uptime_secs,
        "tld": report.tld,
        "http": port_status(report.http),
        "https": port_status(report.https),
        "dns_addr": report.dns_addr.to_string(),
        "resolver_installed": report.resolver_installed,
        "ca_trusted_system": report.ca.trusted_system,
        "default_php": report.default_php.to_string(),
        "php": php,
        "sites": {
            "parked": report.sites.parked,
            "linked": report.sites.linked,
            "secured": report.sites.secured,
        },
        "services": services,
        "mail": report.mail.as_ref().map(|m| json!({
            "enabled": m.enabled,
            "listening": m.listening,
            "port": m.port,
            "unread": m.unread,
        })),
        "symlink_protection": report.symlink_protection,
        "mcp_enabled": report.mcp_enabled,
        "web_unbound": report.web_unbound.as_ref().map(|w| json!({
            "http": w.http,
            "https": w.https,
        })),
        "dns_unbound": report.dns_unbound,
        "shadows": to_value(&report.shadows),
    })
}

fn port_status(port: yerd_ipc::PortStatus) -> Value {
    json!({
        "requested": port.requested,
        "bound": port.bound,
        "fell_back": port.fell_back,
    })
}

/// Serialise without a failure path: the values reaching this are plain data
/// (no non-string map keys, no non-finite floats), so `to_value` cannot fail.
fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn encode(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
}
