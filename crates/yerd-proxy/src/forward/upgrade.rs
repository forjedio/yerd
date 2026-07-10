//! `Connection: Upgrade` bidirectional tunnel for `FrankenPhp` backends.
//!
//! Hyper-1 pattern:
//!   1. `let on_client = hyper::upgrade::on(req);` (consumes the request).
//!   2. Open the backend connection via raw `http1::handshake`, send
//!      the request, await the response.
//!   3. Return the 101 (with `Empty<Bytes>`) from the service so hyper
//!      flushes it to the client.
//!   4. `let on_backend = hyper::upgrade::on(backend_resp);` after the
//!      response is in flight, then `try_join!` both upgrade futures.
//!   5. Wrap each `Upgraded` in `TokioIo` (it implements `hyper::rt`,
//!      not `tokio::io`), then `copy_bidirectional`.

use std::io;
use std::net::SocketAddr;

use http::header::{CONNECTION, UPGRADE};
use http::{HeaderMap, HeaderValue};
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::upgrade::Upgraded;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use crate::error::ProxyError;
use crate::forward::{empty_body, BoxBody};

/// Detect `Connection: Upgrade` per RFC 9110 §7.8.
///
/// `Connection` may carry comma-separated tokens (`keep-alive, upgrade`);
/// the `upgrade` token is case-insensitive.
#[must_use]
pub fn is_upgrade(headers: &HeaderMap) -> bool {
    if !headers.contains_key(UPGRADE) {
        return false;
    }
    headers.get_all(CONNECTION).iter().any(|v| {
        v.to_str().ok().is_some_and(|s| {
            s.split(',')
                .any(|tok| tok.trim().eq_ignore_ascii_case("upgrade"))
        })
    })
}

/// Forward an upgrade request to a FrankenPHP backend and run the
/// bidirectional tunnel.
pub async fn forward(
    req: Request<Incoming>,
    addr: SocketAddr,
) -> Result<Response<BoxBody>, ProxyError> {
    let tcp = TcpStream::connect(addr)
        .await
        .map_err(|source| ProxyError::BackendConnect {
            backend: format!("franken:{addr}"),
            source,
        })?;
    tunnel_over(req, TokioIo::new(tcp)).await
}

/// Run an upgrade (websocket) tunnel over an already-connected `io` (plain TCP
/// or TLS). Shared by the FrankenPHP path ([`forward`]) and the reverse-proxy
/// path (`crate::forward::proxy`). The upstream request is sent with an empty
/// body (an upgrade handshake carries none) and the original `Upgrade` /
/// `Connection` headers preserved so the peer completes the switch.
///
/// If the upstream declines the upgrade (any status other than `101`, e.g.
/// Reverb's `403` + JSON error body for a bad `wss` handshake), its real
/// response is returned with the body streamed through and hop-by-hop stripped
/// without the tunnel's `Connection: upgrade` re-stamp, rather than hijacking a
/// tunnel that would never carry data.
pub(crate) async fn tunnel_over<IO>(
    mut req: Request<Incoming>,
    io: IO,
) -> Result<Response<BoxBody>, ProxyError>
where
    IO: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let on_client = hyper::upgrade::on(&mut req);

    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|source| ProxyError::Hyper { source })?;
    let conn = conn.with_upgrades();
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!(
                target: "yerd_proxy::upgrade",
                error = %e,
                "upgrade connection ended"
            );
        }
    });

    let (parts, _body) = req.into_parts();
    let upstream_req = Request::from_parts(parts, Empty::<bytes::Bytes>::new());
    let mut backend_resp = sender
        .send_request(upstream_req)
        .await
        .map_err(|source| ProxyError::Hyper { source })?;

    let on_backend = hyper::upgrade::on(&mut backend_resp);

    let (mut parts, body) = backend_resp.into_parts();

    if parts.status != http::StatusCode::SWITCHING_PROTOCOLS {
        strip_hop_by_hop_only(&mut parts.headers);
        let boxed: BoxBody = body
            .map_err(|e| std::io::Error::other(e.to_string()))
            .boxed();
        return Ok(Response::from_parts(parts, boxed));
    }

    strip_hop_by_hop(&mut parts.headers);
    let resp: Response<BoxBody> = Response::from_parts(parts, empty_body());

    tokio::spawn(async move {
        match tokio::try_join!(on_client, on_backend) {
            Ok((client_upgraded, backend_upgraded)) => {
                if let Err(e) = run_tunnel(client_upgraded, backend_upgraded).await {
                    tracing::debug!(
                        target: "yerd_proxy::upgrade",
                        error = %e,
                        "upgrade tunnel ended with error"
                    );
                }
            }
            Err(e) => {
                tracing::debug!(
                    target: "yerd_proxy::upgrade",
                    error = %e,
                    "upgrade future failed"
                );
            }
        }
    });

    Ok(resp)
}

async fn run_tunnel(client: Upgraded, backend: Upgraded) -> io::Result<()> {
    let mut client_io = TokioIo::new(client);
    let mut backend_io = TokioIo::new(backend);
    tokio::io::copy_bidirectional(&mut client_io, &mut backend_io)
        .await
        .map(|_| ())
}

static HOP_BY_HOP_FIXED: &[&str] = &[
    "connection",
    "proxy-connection",
    "keep-alive",
    "te",
    "transfer-encoding",
    "trailer",
];

/// Strip hop-by-hop headers per RFC 9110 §7.6.1 **without** re-stamping
/// `Connection` (and dropping any `Upgrade`). This is the variant the plain
/// reverse-proxy path uses on both the request and the response: unlike
/// [`strip_hop_by_hop`] (which keeps `Upgrade` and re-adds `Connection:
/// upgrade` for the tunnel), a normal proxied request must not carry
/// `Connection: upgrade`, and an arbitrary upstream's `Transfer-Encoding` /
/// `Keep-Alive` response headers must not leak through hyper's re-framing.
pub(crate) fn strip_hop_by_hop_only(headers: &mut HeaderMap) {
    // `HeaderName::as_str()` is always lowercase, so no per-header allocation is
    // needed; only the `Connection`-value tokens (arbitrary case) are compared
    // case-insensitively.
    let conn_tokens: Vec<&str> = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(',').map(str::trim))
        .collect();
    let to_remove: Vec<http::HeaderName> = headers
        .iter()
        .filter_map(|(name, _)| {
            let lower = name.as_str();
            if lower == "upgrade"
                || HOP_BY_HOP_FIXED.contains(&lower)
                || conn_tokens.iter().any(|t| t.eq_ignore_ascii_case(lower))
            {
                Some(name.clone())
            } else {
                None
            }
        })
        .collect();
    for name in to_remove {
        headers.remove(&name);
    }
}

/// Strip hop-by-hop headers per RFC 9110 §7.6.1.
fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let conn_tokens: Vec<&str> = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(',').map(str::trim))
        .collect();
    let to_remove: Vec<http::HeaderName> = headers
        .iter()
        .filter_map(|(name, _)| {
            let lower = name.as_str();
            if lower == "upgrade" {
                return None;
            }
            if HOP_BY_HOP_FIXED.contains(&lower)
                || conn_tokens.iter().any(|t| t.eq_ignore_ascii_case(lower))
            {
                Some(name.clone())
            } else {
                None
            }
        })
        .collect();
    for name in to_remove {
        headers.remove(&name);
    }
    headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
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

    #[test]
    fn is_upgrade_detects_single_token() {
        let mut h = HeaderMap::new();
        h.insert(UPGRADE, "websocket".parse().unwrap());
        h.insert(CONNECTION, "upgrade".parse().unwrap());
        assert!(is_upgrade(&h));
    }

    #[test]
    fn is_upgrade_detects_comma_listed_token() {
        let mut h = HeaderMap::new();
        h.insert(UPGRADE, "websocket".parse().unwrap());
        h.insert(CONNECTION, "keep-alive, Upgrade".parse().unwrap());
        assert!(is_upgrade(&h));
    }

    #[test]
    fn is_upgrade_requires_upgrade_header() {
        let mut h = HeaderMap::new();
        h.insert(CONNECTION, "upgrade".parse().unwrap());
        assert!(!is_upgrade(&h));
    }

    #[test]
    fn strip_hop_by_hop_removes_fixed_set_keeps_upgrade() {
        let mut h = HeaderMap::new();
        h.insert(UPGRADE, "websocket".parse().unwrap());
        h.insert(CONNECTION, "keep-alive, upgrade".parse().unwrap());
        h.insert(http::header::TRANSFER_ENCODING, "chunked".parse().unwrap());
        h.insert(http::header::CONTENT_TYPE, "text/plain".parse().unwrap());
        strip_hop_by_hop(&mut h);
        assert!(h.get(UPGRADE).is_some());
        assert_eq!(h.get(CONNECTION).unwrap(), "upgrade");
        assert!(h.get(http::header::TRANSFER_ENCODING).is_none());
        assert!(h.get(http::header::CONTENT_TYPE).is_some());
    }

    /// The strip-only variant (used on normal proxied requests/responses) drops
    /// `Upgrade`/`Connection`/`Transfer-Encoding` and does NOT re-stamp
    /// `Connection: upgrade`.
    #[test]
    fn strip_hop_by_hop_only_removes_upgrade_and_connection_no_restamp() {
        let mut h = HeaderMap::new();
        h.insert(UPGRADE, "websocket".parse().unwrap());
        h.insert(CONNECTION, "keep-alive, upgrade".parse().unwrap());
        h.insert(http::header::TRANSFER_ENCODING, "chunked".parse().unwrap());
        h.insert("keep-alive", "timeout=5".parse().unwrap());
        h.insert(http::header::CONTENT_TYPE, "text/plain".parse().unwrap());
        strip_hop_by_hop_only(&mut h);
        assert!(h.get(UPGRADE).is_none());
        assert!(h.get(CONNECTION).is_none());
        assert!(h.get(http::header::TRANSFER_ENCODING).is_none());
        assert!(h.get("keep-alive").is_none());
        assert!(h.get(http::header::CONTENT_TYPE).is_some());
    }

    /// A `Connection`-listed custom token is hop-by-hop and removed, with no
    /// `Connection` header left behind.
    #[test]
    fn strip_hop_by_hop_only_removes_connection_listed_tokens() {
        let mut h = HeaderMap::new();
        h.insert(CONNECTION, "x-custom".parse().unwrap());
        h.insert("x-custom", "drop-me".parse().unwrap());
        h.insert(http::header::CONTENT_TYPE, "text/plain".parse().unwrap());
        strip_hop_by_hop_only(&mut h);
        assert!(h.get("x-custom").is_none());
        assert!(h.get(CONNECTION).is_none());
        assert!(h.get(http::header::CONTENT_TYPE).is_some());
    }

    #[test]
    fn is_upgrade_false_when_connection_lacks_token() {
        let mut h = HeaderMap::new();
        h.insert(UPGRADE, "websocket".parse().unwrap());
        h.insert(CONNECTION, "keep-alive".parse().unwrap());
        assert!(!is_upgrade(&h));
    }

    #[test]
    fn is_upgrade_false_without_connection_header() {
        let mut h = HeaderMap::new();
        h.insert(UPGRADE, "websocket".parse().unwrap());
        assert!(!is_upgrade(&h));
    }

    /// The Connection-listed `x-custom` token is hop-by-hop and stripped.
    #[test]
    fn strip_hop_by_hop_removes_connection_listed_tokens() {
        let mut h = HeaderMap::new();
        h.insert(UPGRADE, "websocket".parse().unwrap());
        h.insert(CONNECTION, "upgrade, x-custom".parse().unwrap());
        h.insert("x-custom", "drop-me".parse().unwrap());
        h.insert(http::header::CONTENT_TYPE, "text/plain".parse().unwrap());
        strip_hop_by_hop(&mut h);
        assert!(h.get("x-custom").is_none());
        assert!(h.get(http::header::CONTENT_TYPE).is_some());
        assert_eq!(h.get(CONNECTION).unwrap(), "upgrade");
    }

    /// A fresh `Connection: upgrade` is always stamped for the client hop.
    #[test]
    fn strip_hop_by_hop_inserts_connection_upgrade_when_absent() {
        let mut h = HeaderMap::new();
        h.insert(http::header::CONTENT_TYPE, "text/plain".parse().unwrap());
        strip_hop_by_hop(&mut h);
        assert_eq!(h.get(CONNECTION).unwrap(), "upgrade");
    }

    #[test]
    fn strip_hop_by_hop_removes_each_fixed_token() {
        let mut h = HeaderMap::new();
        h.insert(http::header::TE, "trailers".parse().unwrap());
        h.insert("proxy-connection", "keep-alive".parse().unwrap());
        h.insert(http::header::TRAILER, "x".parse().unwrap());
        h.insert("keep-alive", "timeout=5".parse().unwrap());
        h.insert(http::header::TRANSFER_ENCODING, "chunked".parse().unwrap());
        strip_hop_by_hop(&mut h);
        assert!(h.get(http::header::TE).is_none());
        assert!(h.get("proxy-connection").is_none());
        assert!(h.get(http::header::TRAILER).is_none());
        assert!(h.get("keep-alive").is_none());
        assert!(h.get(http::header::TRANSFER_ENCODING).is_none());
    }
}
