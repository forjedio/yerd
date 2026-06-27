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
use http_body_util::Empty;
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
    mut req: Request<Incoming>,
    addr: SocketAddr,
) -> Result<Response<BoxBody>, ProxyError> {
    let on_client = hyper::upgrade::on(&mut req);

    let tcp = TcpStream::connect(addr)
        .await
        .map_err(|source| ProxyError::BackendConnect {
            backend: format!("franken:{addr}"),
            source,
        })?;
    let io = TokioIo::new(tcp);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|source| ProxyError::Hyper { source })?;
    let conn = conn.with_upgrades();
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!(
                target: "yerd_proxy::upgrade",
                error = %e,
                "FrankenPhp upgrade connection ended"
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

    let (mut parts, _body) = backend_resp.into_parts();
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

/// Strip hop-by-hop headers per RFC 9110 §7.6.1.
fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let conn_tokens: Vec<String> = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(',').map(|t| t.trim().to_ascii_lowercase()))
        .collect();
    let to_remove: Vec<http::HeaderName> = headers
        .iter()
        .filter_map(|(name, _)| {
            let lower = name.as_str().to_ascii_lowercase();
            if lower == "upgrade" {
                return None;
            }
            if HOP_BY_HOP_FIXED.contains(&lower.as_str()) || conn_tokens.iter().any(|t| t == &lower)
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
}
