//! Model Context Protocol (MCP) server logic for Yerd: the tool catalog, the
//! JSON-RPC state machine, and tool-result rendering.
//!
//! **Layer:** pure. This crate does no I/O, spawns nothing, reads no clock or
//! environment, and pulls in no async runtime. It is a *sans-io* state machine:
//! feed it one newline-delimited JSON-RPC message at a time with
//! [`Server::handle_line`] and it returns an [`Outgoing`] describing what the
//! caller should do. The stdio loop and the daemon exchange live at the binary
//! edge in `bin/yerd` (`yerd mcp`).
//!
//! Every tool maps to exactly one [`yerd_ipc::Request`], so a tool call needs at
//! most one daemon round trip: [`Outgoing::CallDaemon`] hands the caller a
//! [`PendingCall`], and [`PendingCall::complete`] turns the daemon's answer back
//! into the JSON-RPC reply line.
//!
//! # Gating
//!
//! Serving tools is opt-in (`mcp_enabled` in the config, toggled from the GUI).
//! The gate is a *UX* control, not a security boundary: any process running as
//! the user can already open the daemon's IPC socket. Handshakes therefore
//! always succeed - a disabled server that failed `initialize` would be
//! indistinguishable from a broken one in an agent's server list. Instead the
//! availability shows up in `initialize`'s `instructions` and, per call, as an
//! [`Outgoing::PolicyBlocked`] the caller answers with [`Server::gate_reply`].

mod error;
mod protocol;
mod render;
mod tools;

pub use error::ArgError;

/// The MCP protocol revision this server offers when a client requests one it
/// does not support. Kept as its own constant (rather than indexing
/// [`SUPPORTED_PROTOCOL_VERSIONS`]) so the "latest" is a named fact.
pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";

/// Protocol revisions this server accepts, newest first. On `initialize` the
/// client's requested revision is echoed back when it appears here; otherwise
/// [`LATEST_PROTOCOL_VERSION`] is offered and the client decides whether to
/// proceed.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[
    LATEST_PROTOCOL_VERSION,
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
];

/// The `serverInfo.name` reported to clients.
pub const SERVER_NAME: &str = "yerd";

/// A JSON-RPC request id. Kept as a [`serde_json::Value`] because the spec
/// allows both strings and numbers and requires the id to be echoed back
/// unchanged.
pub type RequestId = serde_json::Value;

/// Whether this session may serve tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// The user has enabled Yerd's MCP tools.
    Enabled,
    /// The user has not enabled Yerd's MCP tools.
    Disabled,
    /// The daemon could not be reached, so the setting is not known. Distinct
    /// from [`Availability::Disabled`] so guidance never claims the toggle is
    /// off when the real problem is that Yerd is not running.
    Unknown,
}

/// A `tools/call` that has been parsed and validated into a daemon request, and
/// is waiting on the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCall {
    /// The daemon request to exchange.
    pub request: yerd_ipc::Request,
    id: RequestId,
    tool: &'static str,
}

impl PendingCall {
    /// The catalog name of the tool that produced this call.
    pub fn tool(&self) -> &'static str {
        self.tool
    }

    pub(crate) fn id(&self) -> &RequestId {
        &self.id
    }

    /// Render the daemon's answer into the JSON-RPC reply line for this call.
    ///
    /// `Err` carries human-readable text describing a transport failure (the
    /// daemon was unreachable, timed out, or closed the connection), which is
    /// surfaced to the agent as a failed tool result rather than a protocol
    /// error: the call was well-formed, it just could not be carried out.
    pub fn complete(self, result: Result<yerd_ipc::Response, String>) -> String {
        let content = match result {
            Ok(resp) => render::render(self.tool, &resp),
            Err(text) => render::tool_error(&text),
        };
        protocol::result_reply(&self.id, content)
    }
}

/// What the caller should do with one handled input line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    /// Write this complete JSON-RPC message to stdout (the caller appends the
    /// newline).
    Reply(String),
    /// Nothing to write: the input was a notification, a stray response, or
    /// otherwise ignorable. Notifications must never be answered.
    None,
    /// Exchange this call with the daemon, then hand the answer to
    /// [`PendingCall::complete`] and write the resulting line.
    CallDaemon(PendingCall),
    /// A valid `tools/call` arrived while this session is not
    /// [`Availability::Enabled`].
    ///
    /// The caller should re-check the toggle (the user may have turned it on
    /// since the session started) and call [`Server::set_availability`] with
    /// what it learns. If that leaves the server [`Availability::Enabled`],
    /// dispatch the call as a normal [`Outgoing::CallDaemon`]; otherwise write
    /// [`Server::gate_reply`] for it.
    ///
    /// The guidance is rendered on demand rather than carried here so it always
    /// describes the *current* reason: a session that started with an
    /// unreachable daemon, then reached one and found the toggle off, must stop
    /// blaming the daemon.
    PolicyBlocked(PendingCall),
}

/// The MCP server state machine.
#[derive(Debug, Clone)]
pub struct Server {
    initialized: bool,
    availability: Availability,
    version: String,
}

impl Server {
    /// Build a server for one session. `server_version` is reported as
    /// `serverInfo.version` (the `yerd` binary's version).
    pub fn new(availability: Availability, server_version: impl Into<String>) -> Self {
        Self {
            initialized: false,
            availability,
            version: server_version.into(),
        }
    }

    /// Update the gate state, e.g. after re-reading the daemon's status
    /// following an [`Outgoing::PolicyBlocked`].
    pub fn set_availability(&mut self, availability: Availability) {
        self.availability = availability;
    }

    /// The current gate state.
    pub fn availability(&self) -> Availability {
        self.availability
    }

    /// Handle one newline-delimited JSON-RPC message.
    pub fn handle_line(&mut self, line: &str) -> Outgoing {
        protocol::handle_line(self, line)
    }

    /// The JSON-RPC reply telling an agent why a gated call was not run, under
    /// the availability set right now. Answers an [`Outgoing::PolicyBlocked`]
    /// once the caller has re-checked the toggle and applied the result with
    /// [`Server::set_availability`].
    pub fn gate_reply(&self, call: &PendingCall) -> String {
        protocol::gate_reply(self.availability, call)
    }
}

/// A JSON-RPC parse error (`-32700`, null id) for input the caller rejected
/// before it could reach [`Server::handle_line`] - e.g. a line past the
/// caller's length cap. Keeps reply construction in one place.
pub fn parse_error_reply(message: &str) -> String {
    protocol::parse_error_reply(message)
}
