//! FastCGI forwarder: connect, split the socket, and write the request on one
//! half while reading the response on the other.
//!
//! The response head goes out as soon as the CGI header block completes;
//! STDOUT records after it are relayed to the client one at a time, so a
//! streamed PHP response (SSE, Livewire `wire:stream`) reaches the browser as
//! PHP flushes it. Two consequences worth knowing:
//!
//! * A failure *after* the head is on the wire cannot change the status, so it
//!   aborts the connection instead. Failures before it still return a
//!   `ProxyError` and get the usual clean 5xx.
//! * A response hyper cannot give a body to (HEAD, 1xx, 204, 304) is drained
//!   here rather than streamed. Hyper drops such a body the instant the head is
//!   encoded, which would otherwise read as a client disconnect and kill the
//!   PHP script mid-run.

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body, Frame};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Request, Response};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::backend::Backend;
use crate::error::ProxyError;
use crate::forward::{empty_body, BoxBody};
use crate::pure::cgi_head::{Head, HeadAccumulator, HeadFeed};
use crate::pure::cgi_params::{build_params, AutoLoginParams};
use crate::pure::fcgi_codec::{
    encode_begin_request_body, encode_name_value, FcgiError, Header, RecordType, FCGI_MAX_PAYLOAD,
    FCGI_RESPONDER, FCGI_VERSION,
};

const REQUEST_ID: u16 = 1;

/// STDOUT records that may sit between the backend reader and the client.
///
/// This bound *is* the backpressure onto FPM: when the client stops consuming,
/// the channel fills, the reader stops reading the socket, the kernel buffer
/// fills, and FPM's own writes block. Unbounded would reintroduce whole-body
/// buffering for slow clients.
const STDOUT_CHANNEL_RECORDS: usize = 8;

/// Response header nginx consumes rather than forwarding, so neither do we.
const X_ACCEL_BUFFERING: &str = "x-accel-buffering";

/// Forward `req` to a FastCGI `backend`, streaming the response.
///
/// Returns once the CGI header block is parsed - the body keeps arriving
/// afterwards through a background reader task - or a `ProxyError` if the
/// backend fails before that point.
///
/// `script_rel`, if given, is a real, on-disk `.php` file (relative to
/// `served_root`) that [`crate::forward::script_file::resolve_script`]
/// resolved for this request - see `pure::cgi_params`'s module doc for the
/// front-controller policy this drives. `auto_login`, if given, is passed
/// straight through to [`build_params`] - see [`AutoLoginParams`] for
/// when/why.
#[allow(clippy::too_many_arguments)]
pub async fn forward(
    req: Request<Incoming>,
    backend: Backend,
    served_root: PathBuf,
    script_rel: Option<PathBuf>,
    server_addr: SocketAddr,
    peer_addr: SocketAddr,
    https: bool,
    auto_login: Option<AutoLoginParams<'_>>,
) -> Result<Response<BoxBody>, ProxyError> {
    let backend_label = backend.to_string();
    let (parts, body) = req.into_parts();

    let stream = open_backend(&backend)
        .await
        .map_err(|source| ProxyError::BackendConnect {
            backend: backend_label.clone(),
            source,
        })?;
    let (mut read_half, write_half) = tokio::io::split(stream);

    let mut prelude: Vec<u8> = Vec::with_capacity(64);
    write_record(
        &mut prelude,
        RecordType::BeginRequest,
        &encode_begin_request_body(FCGI_RESPONDER, false),
    );

    let params = build_params(
        parts.method.as_str(),
        path_and_query_of(&parts.uri),
        &parts.headers,
        &served_root,
        script_rel.as_deref(),
        https,
        peer_addr,
        server_addr,
        auto_login,
    );
    let mut param_buf: Vec<u8> = Vec::new();
    for (name, value) in &params {
        encode_name_value(name, value, &mut param_buf)?;
    }
    for chunk in param_buf.chunks(FCGI_MAX_PAYLOAD) {
        write_record(&mut prelude, RecordType::Params, chunk);
    }
    write_record(&mut prelude, RecordType::Params, &[]);

    let writer_label = backend_label.clone();
    let writer = AbortOnDrop(tokio::spawn(async move {
        if let Err(e) = write_request(write_half, prelude, body, &writer_label).await {
            tracing::debug!(
                target: "yerd_proxy::fcgi",
                backend = %writer_label,
                error = %e,
                "FPM request write ended"
            );
        }
    }));

    let mut accumulator = HeadAccumulator::new();
    let parsed = loop {
        let (header, content) = read_record(&mut read_half).await?;
        match header.record_type {
            RecordType::Stdout => match accumulator.feed(&content) {
                HeadFeed::Pending => {}
                HeadFeed::Complete(head) => break Some(head),
                HeadFeed::TooLarge => {
                    return Err(ProxyError::BackendProtocol {
                        source: io::Error::other("FPM response head exceeded the size limit"),
                    })
                }
            },
            RecordType::Stderr => log_stderr(&backend_label, &content),
            RecordType::EndRequest => break None,
            _ => {}
        }
    };
    let Some(mut head) = parsed else {
        return synthesise_response(accumulator.finish(), empty_body());
    };

    if !can_have_body(&parts.method, head.status) {
        drain_to_end_request(&mut read_half, &backend_label).await?;
        return synthesise_response(head, empty_body());
    }

    let (tx, rx) = mpsc::channel(STDOUT_CHANNEL_RECORDS);
    let remainder = std::mem::take(&mut head.body_remainder);
    tokio::spawn(read_response(
        read_half,
        tx,
        remainder,
        writer,
        backend_label,
    ));
    synthesise_response(head, ChannelBody { rx }.boxed())
}

/// Whether hyper will let this response carry a body. Mirrors hyper's own rule
/// (`proto::h1::role::Server::can_have_body`): it drops the body of anything
/// else the moment the head is encoded, and on the streaming path that drop is
/// indistinguishable from the client hanging up.
fn can_have_body(method: &http::Method, status: http::StatusCode) -> bool {
    if *method == http::Method::HEAD {
        return false;
    }
    if *method == http::Method::CONNECT && status.is_success() {
        return false;
    }
    !(status.is_informational()
        || status == http::StatusCode::NO_CONTENT
        || status == http::StatusCode::NOT_MODIFIED)
}

/// Read records to `END_REQUEST`, discarding STDOUT and logging STDERR.
///
/// Used for responses that cannot carry a body: the client gets the head, and
/// PHP still runs to completion exactly as it did before the forwarder
/// streamed. Nothing has reached the client yet, so an error here still maps to
/// a clean 5xx.
async fn drain_to_end_request(
    read_half: &mut ReadHalf<BackendStream>,
    backend_label: &str,
) -> Result<(), ProxyError> {
    loop {
        let (header, content) = read_record(read_half).await?;
        match header.record_type {
            RecordType::Stderr => log_stderr(backend_label, &content),
            RecordType::EndRequest => return Ok(()),
            _ => {}
        }
    }
}

/// Relay STDOUT records to the client until `END_REQUEST` or the client leaves.
///
/// Owns the writer guard, so finishing here also tears down the request writer,
/// drops both socket halves, and lets FPM abort the request and free its
/// worker.
async fn read_response(
    mut read_half: ReadHalf<BackendStream>,
    tx: mpsc::Sender<Result<Frame<Bytes>, io::Error>>,
    body_remainder: Vec<u8>,
    _writer: AbortOnDrop,
    backend_label: String,
) {
    if !body_remainder.is_empty()
        && tx
            .send(Ok(Frame::data(Bytes::from(body_remainder))))
            .await
            .is_err()
    {
        return;
    }
    loop {
        let record = tokio::select! {
            record = read_record(&mut read_half) => record,
            () = tx.closed() => break,
        };
        let (header, content) = match record {
            Ok(record) => record,
            Err(e) => {
                let _ = tx.send(Err(io::Error::other(e.to_string()))).await;
                break;
            }
        };
        match header.record_type {
            RecordType::Stdout => {
                if content.is_empty() {
                    continue;
                }
                if tx
                    .send(Ok(Frame::data(Bytes::from(content))))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            RecordType::Stderr => log_stderr(&backend_label, &content),
            RecordType::EndRequest => break,
            _ => {}
        }
    }
}

/// Write the prelude, then stream the request `body` as STDIN records (each
/// chunked at `FCGI_MAX_PAYLOAD`), then the zero-length STDIN terminator.
///
/// Runs concurrently with the response read, so a large request body can no
/// longer deadlock against a backend that answers before draining STDIN. HTTP
/// trailers are dropped - FastCGI cannot represent them.
async fn write_request(
    mut write_half: WriteHalf<BackendStream>,
    prelude: Vec<u8>,
    mut body: Incoming,
    backend_label: &str,
) -> io::Result<()> {
    write_half.write_all(&prelude).await?;
    loop {
        match body.frame().await {
            None => break,
            Some(Err(source)) => return Err(io::Error::other(source.to_string())),
            Some(Ok(frame)) => {
                if frame.is_trailers() {
                    tracing::debug!(
                        target: "yerd_proxy::fcgi",
                        backend = %backend_label,
                        "dropping HTTP trailers - FCGI cannot represent them"
                    );
                    continue;
                }
                let Ok(data) = frame.into_data() else {
                    continue;
                };
                for chunk in data.chunks(FCGI_MAX_PAYLOAD) {
                    let mut buf = Vec::with_capacity(8 + chunk.len());
                    write_record(&mut buf, RecordType::Stdin, chunk);
                    write_half.write_all(&buf).await?;
                }
            }
        }
    }
    let mut term = Vec::with_capacity(8);
    write_record(&mut term, RecordType::Stdin, &[]);
    write_half.write_all(&term).await
}

/// Read one whole FCGI record: header, content, and any padding.
///
/// Not cancel-safe - a record can be lost part-read. The only place it is
/// raced is the reader loop's teardown branch, where the connection is being
/// dropped anyway.
async fn read_record(
    read_half: &mut ReadHalf<BackendStream>,
) -> Result<(Header, Vec<u8>), ProxyError> {
    let mut header_buf = [0u8; 8];
    read_half
        .read_exact(&mut header_buf)
        .await
        .map_err(|source| ProxyError::BackendProtocol { source })?;
    let header = Header::decode(&header_buf)?;
    if header.request_id != REQUEST_ID {
        return Err(ProxyError::Fcgi {
            source: FcgiError::UnexpectedRequestId(header.request_id),
        });
    }
    let mut content = vec![0u8; header.content_length as usize];
    read_half
        .read_exact(&mut content)
        .await
        .map_err(|source| ProxyError::BackendProtocol { source })?;
    if header.padding_length > 0 {
        let mut pad = vec![0u8; header.padding_length as usize];
        read_half
            .read_exact(&mut pad)
            .await
            .map_err(|source| ProxyError::BackendProtocol { source })?;
    }
    Ok((header, content))
}

fn log_stderr(backend_label: &str, content: &[u8]) {
    if content.is_empty() {
        return;
    }
    tracing::warn!(
        target: "yerd_proxy::fcgi",
        backend = %backend_label,
        stderr = %String::from_utf8_lossy(content),
        "FPM stderr"
    );
}

/// Build the HTTP response from the parsed CGI head and a body.
///
/// Header names/values that aren't valid HTTP are skipped, as is
/// `X-Accel-Buffering`. A PHP-supplied `Content-Length` passes through
/// verbatim; without one, hyper frames the streaming body as chunked.
fn synthesise_response(head: Head, body: BoxBody) -> Result<Response<BoxBody>, ProxyError> {
    let mut resp = Response::builder().status(head.status);
    if let Some(resp_headers) = resp.headers_mut() {
        for (name, value) in head.headers {
            if name.eq_ignore_ascii_case(X_ACCEL_BUFFERING) {
                continue;
            }
            if let (Ok(n), Ok(v)) = (
                http::HeaderName::from_bytes(name.as_bytes()),
                http::HeaderValue::from_bytes(value.as_bytes()),
            ) {
                resp_headers.append(n, v);
            }
        }
    }
    resp.body(body).map_err(|_| ProxyError::BackendProtocol {
        source: io::Error::other("failed to build response"),
    })
}

/// Streaming response body fed by [`read_response`] over a bounded channel.
struct ChannelBody {
    rx: mpsc::Receiver<Result<Frame<Bytes>, io::Error>>,
}

impl Body for ChannelBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        self.get_mut().rx.poll_recv(cx)
    }
}

/// Aborts the request-writing task when dropped.
///
/// `tokio::io::split` keeps the socket alive until *both* halves drop, and
/// dropping a `JoinHandle` detaches rather than cancels. Holding the handle in
/// this guard makes teardown unconditional: the headers-only fallback, a
/// pre-head error return, the reader task finishing, and cancellation of the
/// `forward` future itself all release the write half.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Forward an upgrade request - FastCGI cannot model duplex byte streams,
/// so MVP returns 501 Not Implemented.
pub fn upgrade_not_supported() -> Response<BoxBody> {
    Response::builder()
        .status(http::StatusCode::NOT_IMPLEMENTED)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(crate::forward::bytes_body(
            b"WebSocket upgrade not supported on FastCGI backends.\n",
        ))
        .unwrap_or_else(|_| Response::new(empty_body()))
}

enum BackendStream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
}

impl tokio::io::AsyncRead for BackendStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            #[cfg(unix)]
            Self::Unix(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for BackendStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            #[cfg(unix)]
            Self::Unix(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => std::pin::Pin::new(s).poll_flush(cx),
            #[cfg(unix)]
            Self::Unix(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            #[cfg(unix)]
            Self::Unix(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}

async fn open_backend(backend: &Backend) -> io::Result<BackendStream> {
    match backend {
        Backend::PhpFpmTcp { addr } => Ok(BackendStream::Tcp(TcpStream::connect(addr).await?)),
        #[cfg(unix)]
        Backend::PhpFpm { socket } => Ok(BackendStream::Unix(
            tokio::net::UnixStream::connect(socket).await?,
        )),
        #[cfg(not(unix))]
        Backend::PhpFpm { .. } => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Unix socket FPM not supported on this OS",
        )),
        Backend::FrankenPhp { .. } => unreachable_franken(),
    }
}

#[cold]
fn unreachable_franken() -> io::Result<BackendStream> {
    Err(io::Error::other(
        "FrankenPhp backend reached FastCGI forwarder — dispatch bug",
    ))
}

fn write_record(out: &mut Vec<u8>, record_type: RecordType, content: &[u8]) {
    let len = u16::try_from(content.len()).unwrap_or(u16::MAX);
    let header = Header {
        version: FCGI_VERSION,
        record_type,
        request_id: REQUEST_ID,
        content_length: len,
        padding_length: 0,
    };
    header.encode(out);
    out.extend_from_slice(content);
}

fn path_and_query_of(uri: &http::Uri) -> &str {
    uri.path_and_query().map_or("/", |pq| pq.as_str())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn head(status: http::StatusCode, headers: &[(&str, &str)]) -> Head {
        Head {
            status,
            headers: headers
                .iter()
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect(),
            body_remainder: Vec::new(),
        }
    }

    #[test]
    fn synthesise_response_carries_status_and_headers() {
        let resp = synthesise_response(
            head(http::StatusCode::CREATED, &[("X-Test", "1")]),
            empty_body(),
        )
        .unwrap();
        assert_eq!(resp.status(), http::StatusCode::CREATED);
        assert_eq!(resp.headers().get("X-Test").unwrap(), "1");
    }

    /// A header name with a space is not a valid HTTP token, so it is dropped.
    #[test]
    fn synthesise_response_skips_invalid_header_name() {
        let resp = synthesise_response(
            head(http::StatusCode::OK, &[("Bad Name", "v"), ("Good", "y")]),
            empty_body(),
        )
        .unwrap();
        assert!(resp.headers().get("Good").is_some());
        assert_eq!(resp.headers().len(), 1);
    }

    #[test]
    fn synthesise_response_strips_x_accel_buffering() {
        let resp = synthesise_response(
            head(
                http::StatusCode::OK,
                &[("X-Accel-Buffering", "no"), ("Content-Type", "text/plain")],
            ),
            empty_body(),
        )
        .unwrap();
        assert!(resp.headers().get("x-accel-buffering").is_none());
        assert_eq!(resp.headers().get("Content-Type").unwrap(), "text/plain");
    }

    #[test]
    fn synthesise_response_empty_head_builds() {
        let resp =
            synthesise_response(head(http::StatusCode::NO_CONTENT, &[]), empty_body()).unwrap();
        assert_eq!(resp.status(), http::StatusCode::NO_CONTENT);
        assert!(resp.headers().is_empty());
    }

    #[test]
    fn can_have_body_matches_hypers_rule() {
        let cases = [
            (http::Method::GET, http::StatusCode::OK, true),
            (http::Method::HEAD, http::StatusCode::OK, false),
            (http::Method::GET, http::StatusCode::NO_CONTENT, false),
            (http::Method::GET, http::StatusCode::NOT_MODIFIED, false),
            (http::Method::GET, http::StatusCode::CONTINUE, false),
            (http::Method::CONNECT, http::StatusCode::OK, false),
            (http::Method::CONNECT, http::StatusCode::BAD_GATEWAY, true),
            (http::Method::POST, http::StatusCode::CREATED, true),
        ];
        for (method, status, expected) in cases {
            assert_eq!(
                can_have_body(&method, status),
                expected,
                "{method} {status}"
            );
        }
    }

    #[test]
    fn upgrade_not_supported_is_501_plaintext() {
        let resp = upgrade_not_supported();
        assert_eq!(resp.status(), http::StatusCode::NOT_IMPLEMENTED);
        assert_eq!(
            resp.headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/plain; charset=utf-8")
        );
    }

    #[test]
    fn write_record_frames_header_then_content() {
        let mut out = Vec::new();
        write_record(&mut out, RecordType::Stdout, b"abc");
        assert_eq!(out.len(), 11);
        let header = Header::decode(&out[..8]).unwrap();
        assert_eq!(header.record_type, RecordType::Stdout);
        assert_eq!(header.request_id, REQUEST_ID);
        assert_eq!(header.content_length, 3);
        assert_eq!(header.padding_length, 0);
        assert_eq!(&out[8..], b"abc");
    }

    #[test]
    fn write_record_empty_content_is_terminator() {
        let mut out = Vec::new();
        write_record(&mut out, RecordType::Params, &[]);
        assert_eq!(out.len(), 8);
        assert_eq!(Header::decode(&out).unwrap().content_length, 0);
    }

    #[test]
    fn path_and_query_of_extracts_or_defaults_to_slash() {
        let uri: http::Uri = "http://h/foo?a=1".parse().unwrap();
        assert_eq!(path_and_query_of(&uri), "/foo?a=1");
        let uri: http::Uri = "http://h".parse().unwrap();
        assert_eq!(path_and_query_of(&uri), "/");
    }

    /// `BackendStream` isn't `Debug`, so match rather than `unwrap_err`.
    #[test]
    fn unreachable_franken_returns_error() {
        match unreachable_franken() {
            Err(e) => assert!(e.to_string().contains("dispatch bug")),
            Ok(_) => panic!("expected an error"),
        }
    }
}
