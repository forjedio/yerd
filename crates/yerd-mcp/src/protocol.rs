//! JSON-RPC 2.0 envelope handling and MCP method dispatch.
//!
//! Two rules drive everything here and are easy to get wrong:
//!
//! - **Notifications are never answered.** A message without an `id` is a
//!   notification; unknown ones are ignored rather than erroring, or the server
//!   would emit a reply to a message that has nothing to reply to. A message
//!   without a `method` is a stray *response* and is likewise ignored.
//! - **`-32xxx` is for protocol faults, not policy.** A disabled toggle or an
//!   unreachable daemon is reported as a failed *tool result*, so the handshake
//!   still succeeds and the server does not look broken to the user.
//!
//! `initialize` and `ping` are answerable at any time (the spec allows a
//! pre-handshake ping); only `tools/list` and `tools/call` require that
//! `initialize` has been answered. The `notifications/initialized` message is
//! informational: it gates *server-initiated* requests, of which this server
//! has none, so a client that calls `tools/list` immediately after the
//! `initialize` result still works.

use serde_json::{json, Value};

use crate::{
    Availability, Outgoing, PendingCall, RequestId, Server, LATEST_PROTOCOL_VERSION, SERVER_NAME,
    SUPPORTED_PROTOCOL_VERSIONS,
};

const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const NOT_INITIALIZED: i32 = -32000;

/// Emitted only if serialising a reply we built ourselves somehow fails, which
/// cannot happen for the values this crate constructs (no non-string map keys,
/// no non-finite numbers). Kept so the encode path never panics.
const ENCODE_FAILURE: &str =
    r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"internal encode failure"}}"#;

/// Handle one input line.
///
/// The two early returns are the protocol's silent paths, and both are
/// load-bearing. A message with no `method` is a stray *response*, which must be
/// ignored rather than errored. A message with no `id` is a *notification*: the
/// only one this server understands (`notifications/initialized`) is
/// informational, and answering one it does not understand would itself be a
/// violation - so every notification is consumed silently.
pub(crate) fn handle_line(server: &mut Server, line: &str) -> Outgoing {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Outgoing::Reply(error_reply(
                &Value::Null,
                PARSE_ERROR,
                &format!("parse error: {e}"),
            ))
        }
    };

    if value.is_array() {
        return Outgoing::Reply(error_reply(
            &Value::Null,
            INVALID_REQUEST,
            "batched requests are not supported; send one message per line",
        ));
    }

    let Some(object) = value.as_object() else {
        return Outgoing::Reply(error_reply(
            &Value::Null,
            INVALID_REQUEST,
            "expected a JSON-RPC request object",
        ));
    };

    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return Outgoing::None;
    };

    let Some(id) = object.get("id") else {
        return Outgoing::None;
    };

    let params = object.get("params").cloned().unwrap_or(Value::Null);
    dispatch(server, method, id, &params)
}

fn dispatch(server: &mut Server, method: &str, id: &RequestId, params: &Value) -> Outgoing {
    match method {
        "initialize" => {
            server.initialized = true;
            Outgoing::Reply(result_reply(id, initialize_result(server, params)))
        }
        "ping" => Outgoing::Reply(result_reply(id, json!({}))),
        "tools/list" => {
            if !server.initialized {
                return Outgoing::Reply(not_initialized(id));
            }
            Outgoing::Reply(result_reply(id, crate::tools::list_result()))
        }
        "tools/call" => tools_call(server, id, params),
        _ => Outgoing::Reply(error_reply(
            id,
            METHOD_NOT_FOUND,
            &format!("unknown method `{method}`"),
        )),
    }
}

/// Dispatch a `tools/call`.
///
/// Tool lookup and argument validation run *before* the availability gate: a
/// malformed call is wrong whatever the toggle says, and answering it with
/// policy guidance would send the agent to fix the wrong thing and retry
/// straight back into the same error.
fn tools_call(server: &mut Server, id: &RequestId, params: &Value) -> Outgoing {
    if !server.initialized {
        return Outgoing::Reply(not_initialized(id));
    }
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Outgoing::Reply(error_reply(
            id,
            INVALID_PARAMS,
            "missing tool name in `params.name`",
        ));
    };
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let (tool, request) = match crate::tools::build(name, &args) {
        Ok(built) => built,
        Err(e) => return Outgoing::Reply(error_reply(id, INVALID_PARAMS, &e.to_string())),
    };

    let call = PendingCall {
        request,
        id: id.clone(),
        tool,
    };
    if server.availability == Availability::Enabled {
        return Outgoing::CallDaemon(call);
    }
    Outgoing::PolicyBlocked(call)
}

pub(crate) fn gate_reply(availability: Availability, call: &PendingCall) -> String {
    result_reply(
        call.id(),
        crate::render::tool_error(gate_guidance(availability)),
    )
}

pub(crate) fn parse_error_reply(message: &str) -> String {
    error_reply(&Value::Null, PARSE_ERROR, message)
}

fn initialize_result(server: &Server, params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = match requested {
        Some(v) if SUPPORTED_PROTOCOL_VERSIONS.contains(&v) => v,
        _ => LATEST_PROTOCOL_VERSION,
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": SERVER_NAME, "version": server.version },
        "instructions": instructions(server.availability),
    })
}

/// Session-level guidance, chosen by gate state at startup. Not refreshed
/// mid-session: `initialize` happens once, so an agent that is later unblocked
/// learns about it from the tool result, not from here.
fn instructions(availability: Availability) -> &'static str {
    match availability {
        Availability::Enabled => {
            "Yerd runs local PHP sites on .test domains, plus databases, mail capture and dump \
             telemetry. Tools that start background work (create_site, install_php) return a \
             job_id immediately: poll job_status with the job_id and the returned next_cursor \
             until state is succeeded, failed, or cancelled."
        }
        Availability::Disabled => {
            "Yerd's MCP tools are currently disabled. The user can enable them in Yerd under \
             Settings > General > AI Agents; enabling applies to this session on the next tool \
             call, so there is no need to restart."
        }
        Availability::Unknown => {
            "Yerd's MCP tools could not be checked because the Yerd daemon was not reachable at \
             startup. If Yerd is not running, ask the user to start it (open the Yerd app, or run \
             yerdd); if a tool reports the feature is disabled, ask the user to enable it under \
             Settings > General > AI Agents."
        }
    }
}

/// Per-call guidance when the gate is not open. [`Availability::Enabled`] shares
/// the disabled arm only to keep the match total: an enabled session never
/// blocks a call, so it never reaches here.
fn gate_guidance(availability: Availability) -> &'static str {
    match availability {
        Availability::Enabled | Availability::Disabled => {
            "Yerd's MCP tools are disabled. Ask the user to enable them in Yerd under Settings > \
             General > AI Agents, then retry - the change applies to this session on the next \
             tool call."
        }
        Availability::Unknown => {
            "Yerd's MCP tools are unavailable: the Yerd daemon could not be reached, so the \
             setting could not be read. Ask the user to start Yerd (open the Yerd app, or run \
             yerdd), and to enable Settings > General > AI Agents if it is off."
        }
    }
}

pub(crate) fn result_reply(id: &RequestId, result: Value) -> String {
    encode(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn error_reply(id: &RequestId, code: i32, message: &str) -> String {
    encode(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    }))
}

fn not_initialized(id: &RequestId) -> String {
    error_reply(
        id,
        NOT_INITIALIZED,
        "server not initialized; send `initialize` first",
    )
}

fn encode(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| ENCODE_FAILURE.to_owned())
}
