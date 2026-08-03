//! Unified shutdown future: ctrl_c on every OS, SIGTERM on Unix, and the
//! console-close / shutdown / logoff control signals on Windows (logoff matters
//! for an autostarted daemon, which dies when the user's session ends).

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

    #[cfg(windows)]
    {
        use tokio::signal::windows;

        let mut ctrlc = windows::ctrl_c().ok();
        let mut ctrl_break = windows::ctrl_break().ok();
        let mut ctrl_close = windows::ctrl_close().ok();
        let mut ctrl_shutdown = windows::ctrl_shutdown().ok();
        let mut ctrl_logoff = windows::ctrl_logoff().ok();

        if ctrlc.is_none()
            && ctrl_break.is_none()
            && ctrl_close.is_none()
            && ctrl_shutdown.is_none()
            && ctrl_logoff.is_none()
        {
            tracing::warn!("no Windows console control handlers installed; awaiting Ctrl-C only");
            let _ = tokio::signal::ctrl_c().await;
            tx.send_replace(true);
            return;
        }

        tokio::select! {
            () = recv_or_pending(ctrlc.as_mut().map(tokio::signal::windows::CtrlC::recv)) => {
                tracing::info!("received Ctrl-C");
            }
            () = recv_or_pending(ctrl_break.as_mut().map(tokio::signal::windows::CtrlBreak::recv)) => {
                tracing::info!("received Ctrl-Break");
            }
            () = recv_or_pending(ctrl_close.as_mut().map(tokio::signal::windows::CtrlClose::recv)) => {
                tracing::info!("received console close");
            }
            () = recv_or_pending(ctrl_shutdown.as_mut().map(tokio::signal::windows::CtrlShutdown::recv)) => {
                tracing::info!("received system shutdown");
            }
            () = recv_or_pending(ctrl_logoff.as_mut().map(tokio::signal::windows::CtrlLogoff::recv)) => {
                tracing::info!("received logoff");
            }
        }
        tx.send_replace(true);
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("received Ctrl-C");
        tx.send_replace(true);
    }
}

/// Await an optional signal-`recv` future, or never resolve when it is `None`
/// (that stream's handler failed to install). Lets one `select!` arm cover an
/// optional stream without a shared trait over tokio's four Windows signal types,
/// degrading to whichever installed - the Unix SIGTERM-failure fallback analogue.
#[cfg(windows)]
async fn recv_or_pending<F: std::future::Future<Output = Option<()>>>(fut: Option<F>) {
    match fut {
        Some(f) => {
            f.await;
        }
        None => std::future::pending::<()>().await,
    }
}
