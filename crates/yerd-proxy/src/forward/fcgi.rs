//! FastCGI forwarder: connect → BEGIN_REQUEST → PARAMS → STDIN → drain
//! STDOUT + STDERR → END_REQUEST.

use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Request, Response};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::backend::Backend;
use crate::error::ProxyError;
use crate::forward::{empty_body, BoxBody};
use crate::pure::cgi_params::build_params;
use crate::pure::fcgi_codec::{
    encode_begin_request_body, encode_name_value, FcgiError, Header, RecordType, FCGI_MAX_PAYLOAD,
    FCGI_RESPONDER, FCGI_VERSION,
};

const REQUEST_ID: u16 = 1;

/// Forward `req` to a FastCGI `backend`. Returns the response, or a `ProxyError`.
pub async fn forward(
    req: Request<Incoming>,
    backend: Backend,
    served_root: PathBuf,
    server_addr: SocketAddr,
    peer_addr: SocketAddr,
    https: bool,
) -> Result<Response<BoxBody>, ProxyError> {
    let backend_label = backend.to_string();
    let (parts, body) = req.into_parts();

    let mut stream = open_backend(&backend)
        .await
        .map_err(|source| ProxyError::BackendConnect {
            backend: backend_label.clone(),
            source,
        })?;

    let mut framed: Vec<u8> = Vec::with_capacity(64);
    write_record(
        &mut framed,
        RecordType::BeginRequest,
        &encode_begin_request_body(FCGI_RESPONDER, false),
    );

    let params = build_params(
        parts.method.as_str(),
        path_and_query_of(&parts.uri),
        &parts.headers,
        &served_root,
        https,
        peer_addr,
        server_addr,
    );
    let mut param_buf: Vec<u8> = Vec::new();
    for (name, value) in &params {
        encode_name_value(name, value, &mut param_buf)?;
    }
    for chunk in param_buf.chunks(FCGI_MAX_PAYLOAD) {
        write_record(&mut framed, RecordType::Params, chunk);
    }
    write_record(&mut framed, RecordType::Params, &[]);

    stream
        .write_all(&framed)
        .await
        .map_err(|source| ProxyError::BackendProtocol { source })?;

    write_stdin(&mut stream, body, &backend_label).await?;

    let (stdout, stderr) = read_fcgi_response(&mut stream).await?;

    if !stderr.is_empty() {
        tracing::warn!(
            target: "yerd_proxy::fcgi",
            backend = %backend_label,
            stderr = %String::from_utf8_lossy(&stderr),
            "FPM stderr"
        );
    }

    let (status, headers, body_bytes) = parse_cgi_response(&stdout);
    synthesise_response(status, headers, body_bytes)
}

/// Stream the request `body` to the backend as FCGI STDIN records (each chunked
/// at `FCGI_MAX_PAYLOAD`), then write the zero-length STDIN terminator. HTTP
/// trailers are dropped - FastCGI cannot represent them.
async fn write_stdin(
    stream: &mut BackendStream,
    mut body: Incoming,
    backend_label: &str,
) -> Result<(), ProxyError> {
    loop {
        match body.frame().await {
            None => break,
            Some(Err(source)) => return Err(ProxyError::Hyper { source }),
            Some(Ok(frame)) => {
                if frame.is_trailers() {
                    tracing::debug!(
                        target: "yerd_proxy::fcgi",
                        backend = %backend_label,
                        "dropping HTTP trailers — FCGI cannot represent them"
                    );
                    continue;
                }
                let Ok(data) = frame.into_data() else {
                    continue;
                };
                for chunk in data.chunks(FCGI_MAX_PAYLOAD) {
                    let mut buf = Vec::with_capacity(8 + chunk.len());
                    write_record(&mut buf, RecordType::Stdin, chunk);
                    stream
                        .write_all(&buf)
                        .await
                        .map_err(|source| ProxyError::BackendProtocol { source })?;
                }
            }
        }
    }
    let mut term = Vec::with_capacity(8);
    write_record(&mut term, RecordType::Stdin, &[]);
    stream
        .write_all(&term)
        .await
        .map_err(|source| ProxyError::BackendProtocol { source })
}

/// Drain STDOUT/STDERR records from the backend until END_REQUEST, returning the
/// concatenated `(stdout, stderr)` byte streams. Unknown record types are
/// ignored defensively.
async fn read_fcgi_response(stream: &mut BackendStream) -> Result<(Vec<u8>, Vec<u8>), ProxyError> {
    let mut stdout = Vec::<u8>::new();
    let mut stderr = Vec::<u8>::new();
    loop {
        let mut header_buf = [0u8; 8];
        stream
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
        stream
            .read_exact(&mut content)
            .await
            .map_err(|source| ProxyError::BackendProtocol { source })?;
        if header.padding_length > 0 {
            let mut pad = vec![0u8; header.padding_length as usize];
            stream
                .read_exact(&mut pad)
                .await
                .map_err(|source| ProxyError::BackendProtocol { source })?;
        }
        match header.record_type {
            RecordType::Stdout => stdout.extend_from_slice(&content),
            RecordType::Stderr => stderr.extend_from_slice(&content),
            RecordType::EndRequest => break,
            _ => {}
        }
    }
    Ok((stdout, stderr))
}

/// Build the HTTP response from the parsed CGI status, headers, and body.
/// Header names/values that aren't valid HTTP are skipped.
fn synthesise_response(
    status: http::StatusCode,
    headers: Vec<(String, String)>,
    body_bytes: &[u8],
) -> Result<Response<BoxBody>, ProxyError> {
    let mut resp = Response::builder().status(status);
    if let Some(resp_headers) = resp.headers_mut() {
        for (name, value) in headers {
            if let (Ok(n), Ok(v)) = (
                http::HeaderName::from_bytes(name.as_bytes()),
                http::HeaderValue::from_bytes(value.as_bytes()),
            ) {
                resp_headers.append(n, v);
            }
        }
    }
    let body: BoxBody = if body_bytes.is_empty() {
        empty_body()
    } else {
        http_body_util::Full::new(Bytes::copy_from_slice(body_bytes))
            .map_err(|never| match never {})
            .boxed()
    };
    resp.body(body).map_err(|_| ProxyError::BackendProtocol {
        source: io::Error::other("failed to build response"),
    })
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

/// Parse a CGI-style header block from FCGI STDOUT. The block ends at the
/// first `\r\n\r\n` or `\n\n`; everything after is the response body.
/// `Status: NNN Reason` is translated into the HTTP status code; absent →
/// 200 OK.
fn parse_cgi_response(stdout: &[u8]) -> (http::StatusCode, Vec<(String, String)>, &[u8]) {
    let split = find_header_terminator(stdout);
    let (head, body) = stdout.split_at(split.0);
    let body = body.get(split.1..).unwrap_or(&[]);
    let head_str = std::str::from_utf8(head).unwrap_or("");

    let mut status = http::StatusCode::OK;
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in head_str.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("Status") {
            if let Some(sc) = parse_cgi_status(value) {
                status = sc;
            }
        } else {
            headers.push((name.to_owned(), value.to_owned()));
        }
    }
    (status, headers, body)
}

/// Parse a CGI `Status:` header value - `"200 OK"` or a bare `"200"` - into an
/// HTTP status code. Returns `None` when it isn't a valid code (caller keeps the
/// default 200).
fn parse_cgi_status(value: &str) -> Option<http::StatusCode> {
    let code = value.split_once(' ').map_or(value, |(code, _)| code);
    http::StatusCode::from_u16(code.parse::<u16>().ok()?).ok()
}

/// Return `(offset_of_terminator, terminator_length)`. If no terminator is
/// found, returns `(stdout.len(), 0)` - body is then empty.
fn find_header_terminator(stdout: &[u8]) -> (usize, usize) {
    for i in 0..stdout.len() {
        if i + 4 <= stdout.len() && stdout.get(i..i + 4) == Some(b"\r\n\r\n") {
            return (i, 4);
        }
        if i + 2 <= stdout.len() && stdout.get(i..i + 2) == Some(b"\n\n") {
            return (i, 2);
        }
    }
    (stdout.len(), 0)
}

// The `Path` import is referenced via the function signature.
#[allow(dead_code)]
fn _path_referenced(_: &Path) {}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn parse_cgi_status_and_headers() {
        let stdout = b"Status: 404 Not Found\r\nContent-Type: text/plain\r\n\r\nnope";
        let (status, headers, body) = parse_cgi_response(stdout);
        assert_eq!(status, http::StatusCode::NOT_FOUND);
        assert!(headers
            .iter()
            .any(|(k, v)| k == "Content-Type" && v == "text/plain"));
        assert_eq!(body, b"nope");
    }

    #[test]
    fn parse_cgi_default_status_is_200() {
        let stdout = b"Content-Type: text/plain\n\nhello";
        let (status, _, body) = parse_cgi_response(stdout);
        assert_eq!(status, http::StatusCode::OK);
        assert_eq!(body, b"hello");
    }

    #[test]
    fn parse_cgi_no_headers_no_body() {
        let (status, headers, body) = parse_cgi_response(b"");
        assert_eq!(status, http::StatusCode::OK);
        assert!(headers.is_empty());
        assert_eq!(body, b"");
    }

    #[test]
    fn find_header_terminator_prefers_crlf() {
        let s = b"A: B\r\n\r\nbody";
        assert_eq!(find_header_terminator(s), (4, 4));
    }

    #[test]
    fn find_header_terminator_falls_back_to_lf() {
        let s = b"A: B\n\nbody";
        assert_eq!(find_header_terminator(s), (4, 2));
    }

    #[test]
    fn parse_cgi_status_with_reason_phrase() {
        assert_eq!(parse_cgi_status("200 OK"), Some(http::StatusCode::OK));
        assert_eq!(
            parse_cgi_status("301 Moved Permanently"),
            Some(http::StatusCode::MOVED_PERMANENTLY)
        );
    }

    #[test]
    fn parse_cgi_status_bare_code() {
        assert_eq!(parse_cgi_status("404"), Some(http::StatusCode::NOT_FOUND));
    }

    #[test]
    fn parse_cgi_status_invalid_is_none() {
        assert!(parse_cgi_status("").is_none());
        assert!(parse_cgi_status("abc").is_none());
        assert!(parse_cgi_status("999999").is_none());
        assert!(parse_cgi_status("99").is_none());
    }

    /// The Status line drives the response code; it is not echoed as a header.
    #[test]
    fn parse_cgi_response_status_header_not_surfaced() {
        let stdout = b"Status: 301 Moved\r\nLocation: /x\r\n\r\n";
        let (status, headers, body) = parse_cgi_response(stdout);
        assert_eq!(status, http::StatusCode::MOVED_PERMANENTLY);
        assert!(headers.iter().any(|(k, v)| k == "Location" && v == "/x"));
        assert!(!headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("Status")));
        assert_eq!(body, b"");
    }

    #[test]
    fn synthesise_response_carries_status_headers_and_body() {
        let resp = synthesise_response(
            http::StatusCode::CREATED,
            vec![("X-Test".to_owned(), "1".to_owned())],
            b"hello",
        )
        .unwrap();
        assert_eq!(resp.status(), http::StatusCode::CREATED);
        assert_eq!(resp.headers().get("X-Test").unwrap(), "1");
    }

    /// A header name with a space is not a valid HTTP token, so it is dropped.
    #[test]
    fn synthesise_response_skips_invalid_header_name() {
        let resp = synthesise_response(
            http::StatusCode::OK,
            vec![
                ("Bad Name".to_owned(), "v".to_owned()),
                ("Good".to_owned(), "y".to_owned()),
            ],
            b"",
        )
        .unwrap();
        assert!(resp.headers().get("Good").is_some());
        assert_eq!(resp.headers().len(), 1);
    }

    #[test]
    fn synthesise_response_empty_body_builds() {
        let resp = synthesise_response(http::StatusCode::NO_CONTENT, vec![], b"").unwrap();
        assert_eq!(resp.status(), http::StatusCode::NO_CONTENT);
        assert!(resp.headers().is_empty());
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
