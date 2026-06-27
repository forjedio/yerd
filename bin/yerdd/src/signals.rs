//! Unified shutdown future: ctrl_c on every OS, SIGTERM on Unix.

use tokio::sync::watch;

/// Await whichever shutdown signal fires first, then `send_replace(true)`
/// through `tx` so every watcher's `changed().await` resolves.
///
/// Returns once a signal has been observed and the broadcast sent.
pub async fn wait_for_shutdown(tx: watch::Sender<bool>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "SIGTERM handler installation failed");
                let _ = tokio::signal::ctrl_c().await;
                tx.send_replace(true);
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received Ctrl-C");
            }
            _ = term.recv() => {
                tracing::info!("received SIGTERM");
            }
        }
        tx.send_replace(true);
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("received Ctrl-C");
        tx.send_replace(true);
    }
}
