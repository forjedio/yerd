//! `yerd mcp` - serve Yerd's tools to AI agents over MCP on stdio.
//!
//! The protocol itself lives in the pure `yerd-mcp` crate; this module is the
//! I/O edge. It reads newline-delimited JSON-RPC from stdin, hands each line to
//! [`yerd_mcp::Server`], forwards the resulting tool calls to the daemon over
//! the same IPC socket the rest of the CLI uses, and writes replies to stdout.
//!
//! **stdout carries the protocol and nothing else.** Every diagnostic goes to
//! stderr; a stray `println!` here would corrupt the stream and desynchronise
//! the client.
//!
//! Reads block, which is safe here and nowhere else: `yerd` runs a
//! current-thread tokio runtime driving exactly this one task (see
//! `bin/yerd/src/main.rs`), so a blocked read starves nothing. It does mean the
//! session is single-flight - while a daemon exchange is in progress the loop
//! cannot answer a `ping` - which is why every exchange is timeout-bounded and
//! why no tool maps to a long-running daemon operation (`create_site` and
//! `install_php` return a job id immediately and are polled).
//!
//! For the same reason the exchanger is `.await`ed, never `block_on`'d: a nested
//! `block_on` inside the runtime that is already driving this task panics.

use std::future::Future;
use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::time::Duration;

use yerd_ipc::{Request, Response};
use yerd_mcp::{Availability, Outgoing, Server};

use crate::error::ClientError;
use crate::transport;

/// Ceiling for a tool's daemon exchange. The floor is set by `status` and
/// `diagnose`: both run the daemon's synchronous port/resolver probe suite, so
/// they are the slowest mapped operations by a wide margin. (Do not reach for
/// `wp_shim`'s 300ms precedent - that guards a fast `ListSites` on a latency
/// path, and would fail healthy status calls here.) Its elapse fails the user's
/// actual call, so it is generous.
const TOOL_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

/// Ceiling for the startup probe and gate re-polls. Shorter than
/// [`TOOL_EXCHANGE_TIMEOUT`] despite sending the same probe-bound
/// `Request::Status`, because both fail safe: the startup probe falls back to
/// [`Availability::Unknown`], and a gate re-poll falls back to the guidance the
/// agent would have got anyway. Neither loses work, so they trade completeness
/// for not stalling session startup.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Longest input line accepted, well past any real MCP message. A client that
/// exceeds it gets a parse error and the loop resynchronises on the next
/// newline rather than trying to buffer without bound.
const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Serve one MCP session on stdin/stdout until EOF.
pub async fn run() -> ExitCode {
    let availability = probe_availability().await;
    eprintln!(
        "yerd mcp: serving MCP over stdio ({}). Tools are gated by Yerd's Settings > General > AI Agents.",
        match availability {
            Availability::Enabled => "enabled",
            Availability::Disabled => "disabled",
            Availability::Unknown => "daemon unreachable",
        }
    );

    let server = Server::new(availability, env!("CARGO_PKG_VERSION"));
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    run_loop(stdin.lock(), stdout.lock(), server, |request, timeout| {
        exchange(request, timeout)
    })
    .await
}

/// Read the gate state from the daemon. An unreachable or slow daemon yields
/// [`Availability::Unknown`] rather than a guess: the daemon may simply not be
/// running yet, and reporting "disabled" would send the user to a toggle that is
/// probably already on.
async fn probe_availability() -> Availability {
    availability_from(exchange(Request::Status, PROBE_TIMEOUT).await)
}

/// Read the gate out of a `Status` answer. Anything else - a transport failure,
/// a timeout, an unexpected variant - is [`Availability::Unknown`]: the setting
/// was not read, and guessing "disabled" would send the user to a toggle that is
/// probably already on. Shared by the startup probe and the per-call gate
/// re-poll so the two cannot drift.
fn availability_from(result: Result<Response, String>) -> Availability {
    match result {
        Ok(Response::Status { report }) => {
            if report.mcp_enabled {
                Availability::Enabled
            } else {
                Availability::Disabled
            }
        }
        _ => Availability::Unknown,
    }
}

/// One timeout-bounded daemon exchange, with any failure already rendered as
/// the text an agent should read.
async fn exchange(request: Request, timeout: Duration) -> Result<Response, String> {
    match tokio::time::timeout(timeout, transport::exchange(&request)).await {
        Err(_elapsed) => Err(TIMED_OUT.to_owned()),
        Ok(Ok(response)) => Ok(response),
        Ok(Err(e)) => Err(describe(&e)),
    }
}

const TIMED_OUT: &str = "The Yerd daemon accepted the connection but did not respond in time; it may be stuck. Ask the user to check on Yerd (restarting it from the app, or `yerd restart daemon`).";

const NOT_RUNNING: &str = "The Yerd daemon is not running. Ask the user to start Yerd (open the Yerd app, or run `yerdd`), then retry.";

const CLOSED: &str = "The Yerd daemon closed the connection without answering: it may have crashed, or the response may have exceeded Yerd's 16 MiB IPC limit. If this was list_dumps, retry with a recent since_id to fetch a smaller page.";

/// Map a transport failure to the text an agent gets back.
///
/// Two distinct causes hide behind a dropped connection - a crashed daemon and
/// an over-large response - and the client genuinely cannot tell them apart: an
/// oversized reply fails on the daemon's *encode* side, which closes without
/// sending anything. So [`CLOSED`] names both. (A structured daemon-side "too
/// large" error would separate them; that is a daemon change, out of scope here.)
fn describe(error: &ClientError) -> String {
    match error {
        ClientError::ConnectionClosed(_)
        | ClientError::Ipc(
            yerd_ipc::IpcError::Io { .. } | yerd_ipc::IpcError::UnexpectedEof { .. },
        ) => CLOSED.to_owned(),
        e if e.is_daemon_down() => NOT_RUNNING.to_owned(),
        e => format!("Yerd's daemon could not carry out the request: {e}"),
    }
}

/// The session loop. Generic over its I/O and its exchanger so it can be driven
/// in tests with in-memory buffers and a fake daemon.
///
/// The exchanger takes an explicit timeout rather than closing over one, so tool
/// calls and gate re-polls can be bounded differently through a single seam.
/// Note the bound is a plain generic future, not an async closure: those need
/// Rust 1.85 and the pure crates hold a 1.77 MSRV.
///
/// Only [`Outgoing::PolicyBlocked`] - a valid tool call made while the gate is
/// shut - re-polls the daemon. `initialize`, `ping`, and notifications are
/// answered from the pure server alone, so a gated session stays as cheap and as
/// responsive as an open one.
pub async fn run_loop<R, W, F, Fut>(
    mut reader: R,
    mut writer: W,
    mut server: Server,
    mut exchange: F,
) -> ExitCode
where
    R: BufRead,
    W: Write,
    F: FnMut(Request, Duration) -> Fut,
    Fut: Future<Output = Result<Response, String>>,
{
    loop {
        let line = match read_line(&mut reader) {
            Ok(Input::Eof) => break,
            Err(error) => {
                eprintln!("yerd mcp: reading stdin failed: {error}");
                return ExitCode::FAILURE;
            }
            Ok(Input::TooLong) => {
                if write_line(
                    &mut writer,
                    &yerd_mcp::parse_error_reply("message exceeds the maximum line length"),
                )
                .is_err()
                {
                    break;
                }
                continue;
            }
            Ok(Input::Line(line)) => line,
        };

        let outgoing = server.handle_line(&line);
        let reply = match outgoing {
            Outgoing::None => continue,
            Outgoing::Reply(reply) => reply,
            Outgoing::CallDaemon(call) => {
                let result = exchange(call.request.clone(), TOOL_EXCHANGE_TIMEOUT).await;
                call.complete(result)
            }
            Outgoing::PolicyBlocked(call) => {
                if let Some(fresh) = repoll(&mut exchange).await {
                    server.set_availability(fresh);
                }
                if server.availability() == Availability::Enabled {
                    let result = exchange(call.request.clone(), TOOL_EXCHANGE_TIMEOUT).await;
                    call.complete(result)
                } else {
                    server.gate_reply(&call)
                }
            }
        };

        if write_line(&mut writer, &reply).is_err() {
            break;
        }
    }
    ExitCode::SUCCESS
}

/// Re-read the gate state for a blocked call. `None` leaves the session's
/// current availability alone: a re-poll that could not reach the daemon has
/// learned nothing, and must not fabricate a reply of its own - the server
/// authors the guidance, with the call's id.
async fn repoll<F, Fut>(exchange: &mut F) -> Option<Availability>
where
    F: FnMut(Request, Duration) -> Fut,
    Fut: Future<Output = Result<Response, String>>,
{
    match availability_from(exchange(Request::Status, PROBE_TIMEOUT).await) {
        Availability::Unknown => None,
        known => Some(known),
    }
}

/// One line of input, or why there wasn't one.
enum Input {
    Line(String),
    TooLong,
    Eof,
}

/// Read one newline-delimited message, bounded by [`MAX_LINE_BYTES`].
///
/// Invalid UTF-8 is replaced rather than rejected: the lossy text then fails
/// JSON parsing and the client gets a parse error, which is the same outcome by
/// a less abrupt route than tearing down the session over one bad byte.
///
/// The cap counts the message, not its terminator, so a full read that ends in a
/// newline is a [`MAX_LINE_BYTES`]-byte message and is allowed. Only a full read
/// with no newline in it is genuinely over-long - and the rest of that line still
/// has to be drained to find the next message boundary.
fn read_line<R: BufRead>(reader: &mut R) -> std::io::Result<Input> {
    let mut buf = Vec::new();
    let mut limited = std::io::Read::take(&mut *reader, MAX_LINE_BYTES as u64 + 1);
    let read = limited.read_until(b'\n', &mut buf)?;
    if read == 0 {
        return Ok(Input::Eof);
    }
    if read > MAX_LINE_BYTES && !buf.ends_with(b"\n") {
        discard_to_newline(reader)?;
        return Ok(Input::TooLong);
    }
    Ok(Input::Line(String::from_utf8_lossy(&buf).into_owned()))
}

/// Drop everything up to and including the next newline, so the next read starts
/// on a message boundary.
///
/// Deliberately not `read_until` into a throwaway buffer: that would buffer the
/// whole remainder of an oversized line, which is the unbounded allocation
/// [`MAX_LINE_BYTES`] exists to prevent. This consumes in constant memory.
fn discard_to_newline<R: BufRead>(reader: &mut R) -> std::io::Result<()> {
    loop {
        let (consumed, done) = {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                return Ok(());
            }
            match available.iter().position(|b| *b == b'\n') {
                Some(at) => (at + 1, true),
                None => (available.len(), false),
            }
        };
        reader.consume(consumed);
        if done {
            return Ok(());
        }
    }
}

/// Write one protocol message and flush it. Flushing per message is the point:
/// a client blocks waiting for its reply, so a buffered stdout would deadlock
/// the session.
fn write_line<W: Write>(writer: &mut W, line: &str) -> std::io::Result<()> {
    writeln!(writer, "{line}")?;
    writer.flush()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use serde_json::{json, Value};
    use yerd_ipc::StatusReport;

    use super::*;

    /// Records every request the loop makes and the deadline it was given, and
    /// answers from a script.
    #[derive(Default)]
    struct FakeDaemon {
        calls: Rc<RefCell<Vec<(Request, Duration)>>>,
        status_enabled: Rc<RefCell<Vec<bool>>>,
    }

    impl FakeDaemon {
        /// An exchanger closure over this fake. `status_enabled` is consumed one
        /// entry per `Status` request, so a test can script the toggle flipping
        /// mid-session; when it runs out, `Status` fails as if unreachable.
        fn exchanger(
            &self,
        ) -> impl FnMut(Request, Duration) -> std::future::Ready<Result<Response, String>> + '_
        {
            let calls = Rc::clone(&self.calls);
            let status = Rc::clone(&self.status_enabled);
            move |request, timeout| {
                calls.borrow_mut().push((request.clone(), timeout));
                let response = match request {
                    Request::Status => {
                        let mut queue = status.borrow_mut();
                        if queue.is_empty() {
                            return std::future::ready(Err("daemon down".to_owned()));
                        }
                        let enabled = queue.remove(0);
                        Ok(Response::Status {
                            report: Box::new(report_with(enabled)),
                        })
                    }
                    Request::ListSites => Ok(Response::Sites { sites: vec![] }),
                    _ => Ok(Response::Ok),
                };
                std::future::ready(response)
            }
        }

        fn requests(&self) -> Vec<Request> {
            self.calls.borrow().iter().map(|(r, _)| r.clone()).collect()
        }

        /// Every `(request, deadline)` pair, in order.
        fn deadlines(&self) -> Vec<(Request, Duration)> {
            self.calls.borrow().clone()
        }

        fn status_probes(&self) -> usize {
            self.calls
                .borrow()
                .iter()
                .filter(|(r, _)| matches!(r, Request::Status))
                .count()
        }
    }

    fn report_with(mcp_enabled: bool) -> StatusReport {
        StatusReport {
            daemon_pid: 1,
            uptime_secs: 1,
            daemon_rss_bytes: None,
            tld: "test".into(),
            http: yerd_ipc::PortStatus {
                requested: 80,
                bound: 80,
                fell_back: false,
            },
            https: yerd_ipc::PortStatus {
                requested: 443,
                bound: 443,
                fell_back: false,
            },
            dns_addr: "127.0.0.1:1053".parse().unwrap(),
            ca: yerd_ipc::CaStatus {
                path: std::path::PathBuf::from("/x"),
                fingerprint: "ab".repeat(32),
                trusted_system: Some(true),
                php_trusts_ca: None,
                browser_trust: None,
            },
            resolver_installed: Some(true),
            port_redirect: None,
            foreign_web_listener: None,
            resolver_backup: None,
            default_php: yerd_core::PhpVersion::new(8, 4),
            php: vec![],
            sites: yerd_ipc::SiteCounts::default(),
            load_avg: None,
            daemon_version: "2.0.3".into(),
            services: vec![],
            mail: None,
            web_unbound: None,
            dns_unbound: None,
            boot_id: None,
            shared_sites: 0,
            symlink_protection: true,
            shadows: vec![],
            mcp_enabled,
            lan_enabled: false,
            lan_ip: None,
            lan_setup_bound: None,
            port_redirect_targets: None,
            lan_redirect_targets: None,
        }
    }

    fn initialize() -> String {
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": yerd_mcp::LATEST_PROTOCOL_VERSION, "capabilities": {} },
        })
        .to_string()
    }

    fn call(id: i64, tool: &str) -> String {
        json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": tool, "arguments": {} },
        })
        .to_string()
    }

    /// Drive a scripted session and return the stdout lines, parsed.
    async fn session(
        availability: Availability,
        input: &[String],
        fake: &FakeDaemon,
    ) -> Vec<Value> {
        let stdin = format!("{}\n", input.join("\n"));
        let mut stdout: Vec<u8> = Vec::new();
        let _code = run_loop(
            std::io::Cursor::new(stdin.into_bytes()),
            &mut stdout,
            Server::new(availability, "9.9.9"),
            fake.exchanger(),
        )
        .await;
        String::from_utf8(stdout)
            .expect("stdout is UTF-8")
            .lines()
            .map(|l| serde_json::from_str(l).expect("each stdout line is one JSON message"))
            .collect()
    }

    fn text_of(reply: &Value) -> &str {
        reply
            .pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .expect("text content")
    }

    #[tokio::test]
    async fn happy_path_session_answers_each_request_in_order() {
        let fake = FakeDaemon::default();
        let out = session(
            Availability::Enabled,
            &[
                initialize(),
                json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
                json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }).to_string(),
                call(3, "list_sites"),
            ],
            &fake,
        )
        .await;

        assert_eq!(out.len(), 3, "the notification is not answered");
        assert_eq!(out[0]["id"], json!(1));
        assert!(out[0].pointer("/result/serverInfo").is_some());
        assert_eq!(out[1]["id"], json!(2));
        assert!(out[1].pointer("/result/tools").is_some());
        assert_eq!(out[2]["id"], json!(3));
        assert_eq!(out[2].pointer("/result/isError"), Some(&json!(false)));
        assert_eq!(
            fake.requests(),
            vec![Request::ListSites],
            "only the tool call reaches the daemon"
        );
    }

    /// Turning the toggle off must not disturb sessions already using Yerd:
    /// once enabled, a session stops asking. The scripted `false` answers below
    /// would flip it back if anything re-polled.
    #[tokio::test]
    async fn an_enabled_session_never_repolls_the_gate() {
        let fake = FakeDaemon::default();
        *fake.status_enabled.borrow_mut() = vec![false, false];
        let out = session(
            Availability::Enabled,
            &[initialize(), call(2, "list_sites"), call(3, "list_sites")],
            &fake,
        )
        .await;

        assert_eq!(out.len(), 3);
        assert_eq!(fake.status_probes(), 0, "no Status probe was sent");
        assert_eq!(
            fake.requests(),
            vec![Request::ListSites, Request::ListSites]
        );
    }

    #[tokio::test]
    async fn pings_and_notifications_never_touch_the_daemon() {
        let fake = FakeDaemon::default();
        let out = session(
            Availability::Disabled,
            &[
                initialize(),
                json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }).to_string(),
                json!({ "jsonrpc": "2.0", "method": "notifications/cancelled" }).to_string(),
            ],
            &fake,
        )
        .await;

        assert_eq!(
            out.len(),
            2,
            "initialize and ping answered, notification not"
        );
        assert_eq!(out[1]["result"], json!({}));
        assert!(
            fake.requests().is_empty(),
            "a gated session still does not poll for a ping: {:?}",
            fake.requests()
        );
    }

    #[tokio::test]
    async fn enabling_mid_session_lets_the_blocked_call_through() {
        let fake = FakeDaemon::default();
        *fake.status_enabled.borrow_mut() = vec![true];
        let out = session(
            Availability::Disabled,
            &[initialize(), call(2, "list_sites")],
            &fake,
        )
        .await;

        assert_eq!(out[1].pointer("/result/isError"), Some(&json!(false)));
        assert_eq!(
            fake.requests(),
            vec![Request::Status, Request::ListSites],
            "the re-poll runs first, then the call it unblocked"
        );
    }

    #[tokio::test]
    async fn a_still_disabled_call_gets_guidance_and_reaches_no_tool() {
        let fake = FakeDaemon::default();
        *fake.status_enabled.borrow_mut() = vec![false];
        let out = session(
            Availability::Disabled,
            &[initialize(), call(2, "list_sites")],
            &fake,
        )
        .await;

        assert_eq!(out[1]["id"], json!(2));
        assert_eq!(out[1].pointer("/result/isError"), Some(&json!(true)));
        assert!(
            text_of(&out[1]).contains("disabled"),
            "{}",
            text_of(&out[1])
        );
        assert_eq!(
            fake.requests(),
            vec![Request::Status],
            "the tool itself never ran"
        );
    }

    #[tokio::test]
    async fn an_unknown_session_that_finds_the_toggle_off_stops_blaming_the_daemon() {
        let fake = FakeDaemon::default();
        *fake.status_enabled.borrow_mut() = vec![false];
        let out = session(
            Availability::Unknown,
            &[initialize(), call(2, "list_sites")],
            &fake,
        )
        .await;

        let text = text_of(&out[1]);
        assert!(text.contains("disabled"), "{text}");
        assert!(!text.contains("could not be reached"), "{text}");
    }

    /// With no scripted `Status` answers the fake daemon stays unreachable, so
    /// the re-poll learns nothing and must leave the session's guidance alone
    /// rather than inventing a reason.
    #[tokio::test]
    async fn a_failed_repoll_leaves_the_session_unknown_and_says_so() {
        let fake = FakeDaemon::default();
        let out = session(
            Availability::Unknown,
            &[initialize(), call(2, "list_sites")],
            &fake,
        )
        .await;

        assert_eq!(out[1]["id"], json!(2), "one reply, with the call's id");
        assert_eq!(out[1].pointer("/result/isError"), Some(&json!(true)));
        assert!(text_of(&out[1]).contains("could not be reached"));
        assert_eq!(fake.requests(), vec![Request::Status]);
    }

    #[tokio::test]
    async fn a_failing_tool_exchange_renders_as_a_failed_tool_result() {
        let mut stdout: Vec<u8> = Vec::new();
        let stdin = format!("{}\n{}\n", initialize(), call(2, "list_sites"));
        let _ = run_loop(
            std::io::Cursor::new(stdin.into_bytes()),
            &mut stdout,
            Server::new(Availability::Enabled, "9.9.9"),
            |_request, _timeout| std::future::ready(Err(NOT_RUNNING.to_owned())),
        )
        .await;

        let out: Vec<Value> = String::from_utf8(stdout)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(out[1].pointer("/result/isError"), Some(&json!(true)));
        assert_eq!(text_of(&out[1]), NOT_RUNNING);
    }

    #[tokio::test]
    async fn malformed_and_oversized_input_keep_the_session_alive() {
        let fake = FakeDaemon::default();
        let long = "x".repeat(MAX_LINE_BYTES + 10);
        let out = session(
            Availability::Enabled,
            &[
                initialize(),
                "{not json".to_owned(),
                long,
                call(4, "list_sites"),
            ],
            &fake,
        )
        .await;

        assert_eq!(out.len(), 4, "every bad line is answered, none is fatal");
        assert_eq!(out[1].pointer("/error/code"), Some(&json!(-32700)));
        assert_eq!(out[2].pointer("/error/code"), Some(&json!(-32700)));
        assert!(
            out[2].pointer("/error/message").is_some_and(|m| m
                .as_str()
                .is_some_and(|s| s.contains("maximum line length"))),
            "the oversized line is named as such: {:?}",
            out[2]
        );
        assert_eq!(out[3]["id"], json!(4), "the session resynchronises");
    }

    #[tokio::test]
    async fn invalid_utf8_is_a_parse_error_not_a_torn_down_session() {
        let mut stdin: Vec<u8> = Vec::new();
        stdin.extend_from_slice(initialize().as_bytes());
        stdin.push(b'\n');
        stdin.extend_from_slice(&[0xff, 0xfe, b'\n']);
        stdin.extend_from_slice(call(3, "list_sites").as_bytes());
        stdin.push(b'\n');

        let fake = FakeDaemon::default();
        let mut stdout: Vec<u8> = Vec::new();
        let _ = run_loop(
            std::io::Cursor::new(stdin),
            &mut stdout,
            Server::new(Availability::Enabled, "9.9.9"),
            fake.exchanger(),
        )
        .await;

        let out: Vec<Value> = String::from_utf8(stdout)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(out.len(), 3);
        assert_eq!(out[1].pointer("/error/code"), Some(&json!(-32700)));
        assert_eq!(out[2]["id"], json!(3));
    }

    /// The exchanger takes a deadline per call rather than closing over one
    /// solely so these two can differ - a tool call is the user's actual work
    /// and gets room, a gate re-poll fails safe and must not stall the session.
    /// Without this the seam's whole rationale is untested and the constants
    /// could be swapped unnoticed.
    #[tokio::test]
    async fn tool_calls_and_gate_repolls_get_different_deadlines() {
        let fake = FakeDaemon::default();
        *fake.status_enabled.borrow_mut() = vec![true];
        let _ = session(
            Availability::Disabled,
            &[initialize(), call(2, "list_sites")],
            &fake,
        )
        .await;

        assert_eq!(
            fake.deadlines(),
            vec![
                (Request::Status, PROBE_TIMEOUT),
                (Request::ListSites, TOOL_EXCHANGE_TIMEOUT),
            ]
        );
        assert!(
            PROBE_TIMEOUT < TOOL_EXCHANGE_TIMEOUT,
            "a re-poll must not be given longer than the call it is gating"
        );
    }

    #[test]
    fn availability_is_only_known_from_a_real_status_answer() {
        assert_eq!(
            availability_from(Ok(Response::Status {
                report: Box::new(report_with(true))
            })),
            Availability::Enabled
        );
        assert_eq!(
            availability_from(Ok(Response::Status {
                report: Box::new(report_with(false))
            })),
            Availability::Disabled
        );
        assert_eq!(
            availability_from(Err(NOT_RUNNING.to_owned())),
            Availability::Unknown,
            "an unreachable daemon is not a disabled toggle"
        );
        assert_eq!(
            availability_from(Err(TIMED_OUT.to_owned())),
            Availability::Unknown
        );
        assert_eq!(
            availability_from(Ok(Response::Ok)),
            Availability::Unknown,
            "an answer that is not a status tells us nothing about the gate"
        );
    }

    /// The oversized-line resync must work on a reader that refills in chunks,
    /// which is the real one: stdin arrives in pipe-sized reads, not all at
    /// once. `Cursor` hands over the whole buffer in one `fill_buf`, so every
    /// other test skips the loop that matters here.
    #[tokio::test]
    async fn resync_works_on_a_chunked_reader() {
        let fake = FakeDaemon::default();
        let long = "x".repeat(MAX_LINE_BYTES + 4096);
        let stdin = format!("{}\n{}\n{}\n", initialize(), long, call(3, "list_sites"));
        let mut stdout: Vec<u8> = Vec::new();
        let _ = run_loop(
            std::io::BufReader::with_capacity(64, std::io::Cursor::new(stdin.into_bytes())),
            &mut stdout,
            Server::new(Availability::Enabled, "9.9.9"),
            fake.exchanger(),
        )
        .await;

        let out: Vec<Value> = String::from_utf8(stdout)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(out.len(), 3);
        assert_eq!(out[1].pointer("/error/code"), Some(&json!(-32700)));
        assert_eq!(
            out[2]["id"],
            json!(3),
            "the message after an oversized one must still be answered"
        );
    }

    /// Build a `job_status` call whose serialised line is exactly `target`
    /// bytes, by padding the job id.
    fn call_of_length(target: usize) -> String {
        let line = |id: &str| {
            json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "job_status", "arguments": { "job_id": id } },
            })
            .to_string()
        };
        let overhead = line("").len();
        let padded = line(&"j".repeat(target - overhead));
        assert_eq!(padded.len(), target, "fixture is the length it claims");
        padded
    }

    /// A line of exactly the cap is legal; one byte more is not. Guards the
    /// `read > MAX_LINE_BYTES` comparison against an off-by-one in either
    /// direction.
    #[tokio::test]
    async fn the_line_cap_is_inclusive() {
        for (length, expect_ok) in [(MAX_LINE_BYTES, true), (MAX_LINE_BYTES + 1, false)] {
            let fake = FakeDaemon::default();
            let out = session(
                Availability::Enabled,
                &[initialize(), call_of_length(length)],
                &fake,
            )
            .await;
            let rejected = out[1]
                .pointer("/error/message")
                .and_then(Value::as_str)
                .is_some_and(|m| m.contains("maximum line length"));
            assert_eq!(
                !rejected, expect_ok,
                "a {length}-byte line (cap is {MAX_LINE_BYTES})"
            );
        }
    }

    #[test]
    fn transport_failures_get_distinct_agent_facing_text() {
        assert_eq!(
            describe(&ClientError::DaemonUnreachable("refused".into())),
            NOT_RUNNING
        );
        assert_eq!(
            describe(&ClientError::ConnectionClosed("closed".into())),
            CLOSED,
            "a dropped connection is not the same as a daemon that never answered"
        );
        assert_eq!(
            describe(&ClientError::Ipc(yerd_ipc::IpcError::UnexpectedEof {
                bytes: 2
            })),
            CLOSED
        );
        assert_eq!(
            describe(&ClientError::Ipc(yerd_ipc::IpcError::Io {
                kind: std::io::ErrorKind::BrokenPipe
            })),
            CLOSED
        );
        let other = describe(&ClientError::Usage("bad".into()));
        assert!(
            other.contains("bad"),
            "the catch-all keeps the detail: {other}"
        );
        assert_ne!(other, NOT_RUNNING);
        assert_ne!(other, CLOSED);
    }

    #[test]
    fn the_closed_text_covers_both_causes_a_client_cannot_distinguish() {
        assert!(CLOSED.contains("crashed"));
        assert!(CLOSED.contains("16 MiB"), "names the frame limit");
        assert!(
            CLOSED.contains("since_id"),
            "tells list_dumps how to recover"
        );
    }
}
