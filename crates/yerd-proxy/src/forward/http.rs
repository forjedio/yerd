//! Plain HTTP/1.1 forwarder for `FrankenPhp` backends.
//!
//! Uses raw `hyper::client::conn::http1::handshake` rather than
//! `hyper_util`'s pooled `legacy` client, because the latter has
//! historical gotchas around upgrades and doesn't expose the upgraded
//! socket cleanly.

use std::io;
use std::net::SocketAddr;

use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use crate::error::ProxyError;
use crate::forward::BoxBody;

/// Forward `req` to the FrankenPHP worker at `addr`.
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
    let io = TokioIo::new(tcp);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|source| ProxyError::Hyper { source })?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!(
                target: "yerd_proxy::http",
                error = %e,
                "FrankenPhp backend connection ended"
            );
        }
    });

    let resp = sender
        .send_request(req)
        .await
        .map_err(|source| ProxyError::Hyper { source })?;
    let (parts, body) = resp.into_parts();
    let boxed: BoxBody = body.map_err(|e| io::Error::other(e.to_string())).boxed();
    Ok(Response::from_parts(parts, boxed))
}
