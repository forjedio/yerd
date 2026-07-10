//! Reverse-proxy forwarder for whole-host proxies and path-into-site rules.
//!
//! Forwards a request to an arbitrary `http(s)://host:port` upstream. Handles
//! both normal requests (streaming the original body and response) and
//! `Connection: Upgrade` websockets (via [`crate::forward::upgrade::tunnel_over`]).
//! A dead or unreachable upstream yields `Ok(502 Bad Gateway)` - never an `Err`,
//! which `handle_request` would map to a 500.

use std::net::IpAddr;

use http::header::{CONTENT_TYPE, SERVER};
use http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use yerd_core::UpstreamTarget;

use crate::client_tls::ProxyClientTls;
use crate::error::ProxyError;
use crate::forward::upgrade;
use crate::forward::{bytes_body, empty_body, BoxBody};

/// Forward `req` to `target`. `peer`/`https`/`client_host` seed the
/// `X-Forwarded-*` headers; `client_tls`/`tld` choose the upstream TLS policy.
pub async fn forward(
    mut req: Request<Incoming>,
    target: &UpstreamTarget,
    client_tls: &ProxyClientTls,
    tld: &str,
    peer: IpAddr,
    https: bool,
    client_host: &str,
) -> Result<Response<BoxBody>, ProxyError> {
    add_forwarded_headers(req.headers_mut(), peer, https, client_host);
    let is_upgrade = upgrade::is_upgrade(req.headers());

    let tcp = match TcpStream::connect((target.host(), target.port())).await {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(
                target: "yerd_proxy::proxy",
                error = %e,
                upstream = %target,
                "upstream connect failed"
            );
            return Ok(bad_gateway_response());
        }
    };

    if target.secure() {
        let config = client_tls.config_for(target, tld);
        let connector = TlsConnector::from(config);
        let server_name =
            match rustls::pki_types::ServerName::try_from(target.server_name().to_owned()) {
                Ok(name) => name,
                Err(e) => {
                    tracing::debug!(
                        target: "yerd_proxy::proxy",
                        error = %e,
                        upstream = %target,
                        "invalid upstream server name"
                    );
                    return Ok(bad_gateway_response());
                }
            };
        let tls = match connector.connect(server_name, tcp).await {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(
                    target: "yerd_proxy::proxy",
                    error = %e,
                    upstream = %target,
                    "upstream TLS handshake failed"
                );
                return Ok(bad_gateway_response());
            }
        };
        run(req, TokioIo::new(tls), is_upgrade).await
    } else {
        run(req, TokioIo::new(tcp), is_upgrade).await
    }
}

/// Drive the request over an already-connected `io` (TCP or TLS).
async fn run<IO>(
    req: Request<Incoming>,
    io: IO,
    is_upgrade: bool,
) -> Result<Response<BoxBody>, ProxyError>
where
    IO: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    if is_upgrade {
        upgrade::tunnel_over(req, io).await
    } else {
        forward_plain(req, io).await
    }
}

/// Non-upgrade path: stream the original request body upstream and the response
/// body back, stripping hop-by-hop headers on both sides.
async fn forward_plain<IO>(req: Request<Incoming>, io: IO) -> Result<Response<BoxBody>, ProxyError>
where
    IO: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(io).await {
        Ok(pair) => pair,
        Err(e) => {
            tracing::debug!(target: "yerd_proxy::proxy", error = %e, "upstream handshake failed");
            return Ok(bad_gateway_response());
        }
    };
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!(target: "yerd_proxy::proxy", error = %e, "upstream connection ended");
        }
    });

    let (mut parts, body) = req.into_parts();
    upgrade::strip_hop_by_hop_only(&mut parts.headers);
    let upstream_req = Request::from_parts(parts, body);

    let resp = match sender.send_request(upstream_req).await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(target: "yerd_proxy::proxy", error = %e, "upstream request failed");
            return Ok(bad_gateway_response());
        }
    };
    let (mut rparts, rbody) = resp.into_parts();
    upgrade::strip_hop_by_hop_only(&mut rparts.headers);
    let boxed: BoxBody = rbody
        .map_err(|e| std::io::Error::other(e.to_string()))
        .boxed();
    Ok(Response::from_parts(rparts, boxed))
}

/// Set `X-Forwarded-*` / `X-Real-IP` on the outgoing request. `X-Forwarded-For`
/// appends to any existing chain; the original `Host` header is left untouched
/// (upstreams like Reverb key on it).
fn add_forwarded_headers(headers: &mut HeaderMap, peer: IpAddr, https: bool, client_host: &str) {
    let proto = if https { "https" } else { "http" };
    if let Ok(v) = HeaderValue::from_str(proto) {
        headers.insert(HeaderName::from_static("x-forwarded-proto"), v);
    }
    if let Ok(v) = HeaderValue::from_str(client_host) {
        headers.insert(HeaderName::from_static("x-forwarded-host"), v);
    }
    let peer_str = peer.to_string();
    if let Ok(v) = HeaderValue::from_str(&peer_str) {
        headers.insert(HeaderName::from_static("x-real-ip"), v);
    }
    let xff_name = HeaderName::from_static("x-forwarded-for");
    let chained = match headers.get(&xff_name).and_then(|v| v.to_str().ok()) {
        Some(prev) if !prev.is_empty() => format!("{prev}, {peer_str}"),
        _ => peer_str,
    };
    if let Ok(v) = HeaderValue::from_str(&chained) {
        headers.insert(xff_name, v);
    }
}

/// `502 Bad Gateway` for an unreachable/failed upstream.
fn bad_gateway_response() -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(SERVER, HeaderValue::from_static(yerd_core::PROXY_SERVER_ID))
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(bytes_body(b"Proxy upstream unavailable.\n"))
        .unwrap_or_else(|_| {
            let mut resp = Response::new(empty_body());
            *resp.status_mut() = StatusCode::BAD_GATEWAY;
            resp
        })
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
    fn forwarded_for_appends_to_existing_chain() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-forwarded-for"),
            HeaderValue::from_static("203.0.113.7"),
        );
        add_forwarded_headers(
            &mut headers,
            "198.51.100.2".parse().unwrap(),
            true,
            "app.test",
        );
        assert_eq!(
            headers.get("x-forwarded-for").unwrap(),
            "203.0.113.7, 198.51.100.2"
        );
        assert_eq!(headers.get("x-forwarded-proto").unwrap(), "https");
        assert_eq!(headers.get("x-forwarded-host").unwrap(), "app.test");
        assert_eq!(headers.get("x-real-ip").unwrap(), "198.51.100.2");
    }

    #[test]
    fn forwarded_for_starts_a_fresh_chain() {
        let mut headers = HeaderMap::new();
        add_forwarded_headers(
            &mut headers,
            "198.51.100.2".parse().unwrap(),
            false,
            "app.test",
        );
        assert_eq!(headers.get("x-forwarded-for").unwrap(), "198.51.100.2");
        assert_eq!(headers.get("x-forwarded-proto").unwrap(), "http");
    }

    #[test]
    fn bad_gateway_is_502() {
        assert_eq!(bad_gateway_response().status(), StatusCode::BAD_GATEWAY);
    }
}
