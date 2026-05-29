//! IPC accept loop + per-request dispatch.

use std::sync::Arc;

use interprocess::local_socket::tokio::Listener;
use interprocess::local_socket::tokio::Stream as IpcStream;
use interprocess::local_socket::traits::tokio::Listener as _;
use interprocess::local_socket::traits::tokio::Stream as _;
use tokio::sync::watch;

use yerd_core::SiteRouter;
use yerd_ipc::{
    read_message, write_message, ErrorCode, FrameDecoder, IpcError, Request, Response,
    DEFAULT_MAX_FRAME,
};

/// Run the IPC accept loop until `shutdown_rx` resolves.
pub async fn run(
    listener: Listener,
    router: Arc<SiteRouter>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok(stream) => {
                        let router = router.clone();
                        tokio::spawn(handle_client(stream, router));
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "ipc accept failed");
                    }
                }
            }
        }
    }
}

async fn handle_client(stream: IpcStream, router: Arc<SiteRouter>) {
    let (reader, writer) = stream.split();
    let mut reader = reader;
    let mut writer = writer;
    let mut decoder = FrameDecoder::new();
    loop {
        let req = match read_message::<_, Request>(&mut reader, &mut decoder).await {
            Ok(Some(r)) => r,
            Ok(None) => return,
            Err(e) => {
                // Decode errors close the connection but stay quiet at
                // debug — common with mismatched-version clients.
                if !matches!(e, IpcError::UnexpectedEof { .. }) {
                    tracing::debug!(error = %e, "ipc decode error");
                }
                return;
            }
        };
        let resp = dispatch(req, &router);
        if let Err(e) = write_message(&mut writer, &resp, DEFAULT_MAX_FRAME).await {
            tracing::debug!(error = %e, "ipc write error");
            return;
        }
    }
}

fn dispatch(req: Request, router: &SiteRouter) -> Response {
    match req {
        Request::Ping => Response::Pong,
        Request::ListSites => Response::Sites {
            sites: router.iter().cloned().collect(),
        },
        _ => Response::Error {
            code: ErrorCode::Internal,
            message: "command not implemented in MVP".into(),
        },
    }
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
    use yerd_core::{RouterConfig, Tld};

    fn empty_router() -> SiteRouter {
        SiteRouter::new(RouterConfig::with_tld(Tld::new("test").unwrap()))
    }

    #[test]
    fn dispatch_ping_returns_pong() {
        let router = empty_router();
        let resp = dispatch(Request::Ping, &router);
        assert!(matches!(resp, Response::Pong));
    }

    #[test]
    fn dispatch_list_sites_empty_returns_empty_vec() {
        let router = empty_router();
        let resp = dispatch(Request::ListSites, &router);
        match resp {
            Response::Sites { sites } => assert!(sites.is_empty()),
            other => panic!("expected Sites, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_park_returns_internal_error() {
        let router = empty_router();
        let resp = dispatch(
            Request::Park {
                path: std::path::PathBuf::from("/tmp/x"),
            },
            &router,
        );
        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::Internal);
                assert!(message.contains("not implemented"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }
}
