//! Background supervision of `cloudflared` instances.
//!
//! [`launch`] registers and spawns an instance under a brief manager lock, then
//! hands it to a [`drive`] task that ticks [`yerd_tunnel::TunnelManager::advance`]
//! for the rest of the instance's life, with the lock released between ticks
//! and around every sleep and `/ready` probe. The task is fenced by the
//! generation `begin` returned, so it exits as soon as its instance is stopped
//! or replaced, and it exits when the daemon's shutdown signal trips.
//!
//! IPC handlers that need an answer (a quick share's URL, a named start) wait
//! on [`wait_settled`], which polls snapshots for that same generation only.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tokio::task::JoinHandle;
use yerd_supervise::{HealthProbe, Listen};
use yerd_tunnel::{CloudflaredReadyProbe, LaunchSpec, Step, TunnelError, TunnelKind, TunnelState};

use crate::state::DaemonState;

/// How long a start request waits for its instance to settle.
pub(super) const SETTLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Consecutive failed attempts after which a waiting start gives up early. One
/// transient exit (a metrics-port race, a one-off edge error) is retried by the
/// supervisor and waited through; a deterministic failure such as a broken
/// binary still errors within a few seconds instead of the full timeout.
const SETTLE_MAX_FAILURES: u32 = 3;

/// Snapshot poll interval while waiting for an instance to settle.
const SETTLE_POLL: Duration = Duration::from_millis(250);

/// Pause after an unexpected `advance` error before ticking again.
const ERROR_PAUSE: Duration = Duration::from_secs(1);

/// A freshly started instance: its generation and the driver task supervising
/// it. Production callers drop the handle; tests await it.
pub struct Launched {
    /// The generation `begin` assigned to this start.
    pub generation: u64,
    /// The driver task, which completes once the instance is gone or the daemon
    /// shuts down.
    pub handle: JoinHandle<()>,
}

/// How an instance ended up after a start request waited on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    /// Online (edge reachable); the route map has been rebuilt.
    Running,
    /// Failed, or still not online when the wait timed out.
    NotRunning {
        /// The instance's recorded failure, if any.
        last_error: Option<String>,
    },
    /// The instance was stopped, or replaced by a newer generation.
    Gone,
}

/// Register and spawn `spec` under `key`, then supervise it in the background.
///
/// Returns `Ok(None)` when an instance for `key` is already live or retrying
/// (nothing is restarted), and `Ok(Some(..))` for a fresh start.
///
/// # Errors
///
/// [`TunnelError::ShuttingDown`] once the manager has been shut down, so a late
/// start never leaves an unsupervised `cloudflared` behind.
/// [`TunnelError::NotInstalled`] when `spec.binary` is gone by the time the
/// manager lock is held: callers resolve the binary before taking it, so a
/// `cloudflared` uninstall (which holds the manager lock while it removes the
/// binary) can land in between.
pub async fn launch(
    state: &Arc<DaemonState>,
    key: &str,
    spec: LaunchSpec,
    kind: TunnelKind,
) -> Result<Option<Launched>, TunnelError> {
    let begun = {
        let mut mgr = state.tunnel_manager.lock().await;
        if !spec.binary.is_file() {
            return Err(TunnelError::NotInstalled);
        }
        mgr.begin(key, spec, kind).await?
    };
    Ok(begun.map(|generation| Launched {
        generation,
        handle: tokio::spawn(drive(Arc::clone(state), key.to_owned(), generation)),
    }))
}

/// The per-instance driver loop. See the module doc.
async fn drive(state: Arc<DaemonState>, key: String, generation: u64) {
    let mut shutdown = state.shutdown_tx.subscribe();
    let mut queued: Option<Step> = None;
    loop {
        if *shutdown.borrow() {
            return;
        }
        let step = if let Some(step) = queued.take() {
            Ok(step)
        } else {
            let mut mgr = state.tunnel_manager.lock().await;
            mgr.advance(&key, generation).await
        };
        match step {
            Ok(Step::Continue) => {}
            Ok(Step::Sleep(wait)) => {
                if !pause(&mut shutdown, wait).await {
                    return;
                }
            }
            Ok(Step::Probe(addr)) => {
                let listen = Listen::TcpLoopback(addr);
                let ok = tokio::select! {
                    result = CloudflaredReadyProbe.probe(&listen) => result.is_ok(),
                    () = shutdown_signalled(&mut shutdown) => return,
                };
                let mut mgr = state.tunnel_manager.lock().await;
                queued = Some(mgr.record_probe(&key, generation, ok));
            }
            Ok(Step::Ready) => super::rebuild_routes(&state).await,
            Ok(Step::Gone) => return,
            Err(e) => {
                tracing::warn!(key = %key, error = %e, "tunnel supervision step failed");
                if !pause(&mut shutdown, ERROR_PAUSE).await {
                    return;
                }
            }
        }
    }
}

/// Sleep for `wait`, returning `false` if shutdown tripped first.
async fn pause(shutdown: &mut watch::Receiver<bool>, wait: Duration) -> bool {
    tokio::select! {
        () = tokio::time::sleep(wait) => true,
        () = shutdown_signalled(shutdown) => false,
    }
}

/// Resolve once the shutdown flag is set (or its sender is gone).
async fn shutdown_signalled(shutdown: &mut watch::Receiver<bool>) {
    while !*shutdown.borrow_and_update() {
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

/// The generation of `key`'s current instance, if any.
pub(super) async fn current_generation(state: &DaemonState, key: &str) -> Option<u64> {
    let mut mgr = state.tunnel_manager.lock().await;
    mgr.snapshots()
        .into_iter()
        .find(|s| s.site == key)
        .map(|s| s.generation)
}

/// Wait up to `timeout` for `key`'s instance of `generation` to settle. A
/// missing instance or a different generation is [`Settled::Gone`] at once (it
/// was stopped, or stopped and restarted by someone else). On `Running` the
/// route map is rebuilt first, so a quick tunnel's host is routable before its
/// URL is returned. A `Failed` instance with a retry pending is waited through
/// until it has failed [`SETTLE_MAX_FAILURES`] times in a row, then reported
/// as [`Settled::NotRunning`].
pub async fn wait_settled(
    state: &DaemonState,
    key: &str,
    generation: u64,
    timeout: Duration,
) -> Settled {
    let deadline = Instant::now() + timeout;
    loop {
        let snapshot = {
            let mut mgr = state.tunnel_manager.lock().await;
            mgr.snapshots().into_iter().find(|s| s.site == key)
        };
        let Some(snapshot) = snapshot.filter(|s| s.generation == generation) else {
            return Settled::Gone;
        };
        match snapshot.state {
            TunnelState::Running => {
                super::rebuild_routes(state).await;
                return Settled::Running;
            }
            TunnelState::Failed if snapshot.failures >= SETTLE_MAX_FAILURES => {
                return Settled::NotRunning {
                    last_error: snapshot.last_error,
                }
            }
            TunnelState::Failed | TunnelState::Connecting | TunnelState::Reconnecting => {
                if Instant::now() >= deadline {
                    return Settled::NotRunning {
                        last_error: snapshot.last_error,
                    };
                }
            }
        }
        tokio::time::sleep(SETTLE_POLL).await;
    }
}

/// Resume sharing on daemon start: rebuild the route map from config, then
/// start the named tunnel when it has hostnames to serve and is set up. Quick
/// tunnels are ephemeral and never resumed. `epoch` is the `named_stop_epoch`
/// read when the resume was queued (see [`super::named::resume`]). Errors are
/// logged, not returned.
pub async fn resume(state: Arc<DaemonState>, epoch: u64) {
    super::rebuild_routes(&state).await;
    super::named::resume(state, epoch).await;
}

#[cfg(all(test, unix))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
pub(super) mod tests {
    use super::super::named::NAMED_KEY;
    use super::*;
    use crate::test_support::state_in;
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use yerd_core::{PhpVersion, Site};
    use yerd_ipc::{Response, TunnelRunState};
    use yerd_tunnel::TunnelSnapshot;

    const STUB_HOST: &str = "stub-tunnel.trycloudflare.com";

    pub(crate) const QUICK_STUB: &[u8] =
        b"#!/bin/sh\necho 'INF |  https://stub-tunnel.trycloudflare.com  |' >&2\nexec sleep 60\n";

    const SILENT_STUB: &[u8] = b"#!/bin/sh\nexec sleep 60\n";

    const FAILING_STUB: &[u8] =
        b"#!/bin/sh\necho '2026-01-01T00:00:00Z ERR failed to request quick Tunnel' >&2\nexit 1\n";

    const FLAKY_STUB: &[u8] = b"#!/bin/sh\nif [ ! -f \"$HOME/ran\" ]; then touch \"$HOME/ran\"; echo '2026-01-01T00:00:00Z ERR transient' >&2; exit 1; fi\necho 'INF |  https://stub-tunnel.trycloudflare.com  |' >&2\nexec sleep 60\n";

    const RESHARE_STUB: &[u8] = b"#!/bin/sh\nif [ -f \"$HOME/ran\" ]; then exit 1; fi\ntouch \"$HOME/ran\"\necho 'INF |  https://stub-tunnel.trycloudflare.com  |' >&2\nexec sleep 60\n";

    pub(crate) async fn ready_listener() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 512];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .await;
                });
            }
        });
        addr
    }

    pub(crate) async fn stub_state(tmp: &std::path::Path, stub: &[u8]) -> Arc<DaemonState> {
        let ready = ready_listener().await;
        let mut state = state_in(tmp);
        state.tunnel_origin_port = Some(1);
        state.tunnel_metrics_addr = Arc::new(move || Ok(ready));
        let root = tmp.join("app");
        std::fs::create_dir_all(&root).unwrap();
        let site = Site::linked("app", &root, PhpVersion::new(8, 3)).unwrap();
        state.router.write().await.insert(site).unwrap();
        crate::tools::cloudflared::install_binary_for_test(&state.dirs, "2026.6.1", stub);
        super::super::refresh_cloudflared_cache_after_install(&state).await;
        Arc::new(state)
    }

    async fn launch_stub(state: &Arc<DaemonState>, key: &str) -> Launched {
        let binary = crate::tools::cloudflared::binary_path(&state.dirs);
        let env = super::super::pinned_home_env(state);
        let spec = super::super::launch_spec(state, key, binary, Vec::new(), env).unwrap();
        launch(state, key, spec, TunnelKind::Quick)
            .await
            .unwrap()
            .unwrap()
    }

    pub(crate) async fn snapshots(state: &DaemonState) -> Vec<TunnelSnapshot> {
        state.tunnel_manager.lock().await.snapshots()
    }

    pub(crate) async fn poll_until(
        state: &DaemonState,
        done: impl Fn(&[TunnelSnapshot]) -> bool,
    ) -> Vec<TunnelSnapshot> {
        for _ in 0..200 {
            let snaps = snapshots(state).await;
            if done(&snaps) {
                return snaps;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("tunnel state never reached the expected condition");
    }

    async fn finished(handle: JoinHandle<()>) {
        tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("driver task did not finish")
            .unwrap();
    }

    pub(crate) async fn teardown(state: &DaemonState) {
        let _ = state.tunnel_manager.lock().await.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn quick_share_settles_with_its_url_routed_and_stop_unroutes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let state = stub_state(tmp.path(), QUICK_STUB).await;

        let resp = crate::tunnel::start_quick_tunnel("app", &state).await;

        let Response::Tunnels { tunnels, .. } = resp else {
            panic!("expected Tunnels, got {resp:?}");
        };
        let info = tunnels.iter().find(|t| t.site == "app").unwrap();
        assert_eq!(info.state, TunnelRunState::Running);
        assert_eq!(
            info.url.as_deref(),
            Some("https://stub-tunnel.trycloudflare.com")
        );
        assert!(state.tunnel_routes.read().await.contains_key(STUB_HOST));
        assert_eq!(
            std::fs::read_to_string(state.dirs.data.join("tunnel/quick.yml")).unwrap(),
            yerd_tunnel::config::render_quick_config()
        );

        let stopped = crate::tunnel::stop_tunnel("app", None, &state).await;

        assert!(matches!(stopped, Response::Tunnels { .. }), "{stopped:?}");
        assert!(!state.tunnel_routes.read().await.contains_key(STUB_HOST));
        assert!(snapshots(&state).await.is_empty());
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_driver_finishes_once_its_instance_is_stopped() {
        let tmp = tempfile::tempdir().unwrap();
        let state = stub_state(tmp.path(), QUICK_STUB).await;
        let launched = launch_stub(&state, "app").await;

        let settled = wait_settled(&state, "app", launched.generation, SETTLE_TIMEOUT).await;
        assert_eq!(settled, Settled::Running);
        assert!(state.tunnel_routes.read().await.contains_key(STUB_HOST));

        crate::tunnel::stop_tunnel("app", None, &state).await;

        finished(launched.handle).await;
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stopping_the_named_key_as_a_site_leaves_the_named_tunnel_running() {
        let tmp = tempfile::tempdir().unwrap();
        let state = stub_state(tmp.path(), QUICK_STUB).await;
        launch_stub(&state, NAMED_KEY).await;

        for provider in [
            None,
            Some(yerd_core::tunnel_provider::TunnelProvider::Cloudflare),
        ] {
            crate::tunnel::stop_tunnel(NAMED_KEY, provider, &state).await;
            assert!(snapshots(&state).await.iter().any(|s| s.site == NAMED_KEY));
        }
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cloudflared_uninstall_is_refused_while_a_tunnel_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let state = stub_state(tmp.path(), QUICK_STUB).await;
        let launched = launch_stub(&state, "app").await;

        assert!(matches!(
            crate::tunnel::lock_for_cloudflared_uninstall(&state).await,
            Err(Response::Error {
                code: yerd_ipc::ErrorCode::InvalidPath,
                ..
            })
        ));

        crate::tunnel::stop_tunnel("app", None, &state).await;
        finished(launched.handle).await;
        assert!(crate::tunnel::lock_for_cloudflared_uninstall(&state)
            .await
            .is_ok());
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cloudflared_uninstall_guard_holds_the_manager() {
        let tmp = tempfile::tempdir().unwrap();
        let state = stub_state(tmp.path(), QUICK_STUB).await;

        let guard = crate::tunnel::lock_for_cloudflared_uninstall(&state)
            .await
            .ok()
            .unwrap();
        assert!(state.tunnel_manager.try_lock().is_err());
        drop(guard);
        assert!(state.tunnel_manager.try_lock().is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_launch_resolved_before_an_uninstall_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let state = stub_state(tmp.path(), QUICK_STUB).await;
        let binary = crate::tools::cloudflared::binary_path(&state.dirs);
        let env = super::super::pinned_home_env(&state);
        let spec = super::super::launch_spec(&state, "app", binary, Vec::new(), env).unwrap();
        crate::tools::uninstall(&state.dirs, crate::tools::Tool::Cloudflared).unwrap();

        let result = launch(&state, "app", spec, TunnelKind::Quick).await;
        assert!(matches!(result, Err(TunnelError::NotInstalled)));
        assert!(snapshots(&state).await.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_ends_a_running_driver_and_a_late_driver_returns_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let state = stub_state(tmp.path(), QUICK_STUB).await;
        let running = launch_stub(&state, "app").await;
        assert_eq!(
            wait_settled(&state, "app", running.generation, SETTLE_TIMEOUT).await,
            Settled::Running
        );

        state.shutdown_tx.send_replace(true);
        finished(running.handle).await;

        let late = launch_stub(&state, "late").await;
        finished(late.handle).await;
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resharing_during_backoff_keeps_the_retrying_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let state = stub_state(tmp.path(), RESHARE_STUB).await;
        let first = crate::tunnel::start_quick_tunnel("app", &state).await;
        assert!(matches!(first, Response::Tunnels { .. }), "{first:?}");
        let before = snapshots(&state).await.pop().unwrap();
        let pid = before.pid.unwrap();

        let killed = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .unwrap();
        assert!(killed.success());
        poll_until(&state, |s| {
            s.iter()
                .any(|t| t.site == "app" && t.state == yerd_tunnel::TunnelState::Failed)
        })
        .await;

        let again = crate::tunnel::start_quick_tunnel("app", &state).await;

        assert!(matches!(again, Response::Tunnels { .. }), "{again:?}");
        let after = snapshots(&state).await;
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].generation, before.generation);
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stopping_during_a_pending_start_returns_promptly() {
        let tmp = tempfile::tempdir().unwrap();
        let state = stub_state(tmp.path(), SILENT_STUB).await;
        let starter = {
            let state = Arc::clone(&state);
            tokio::spawn(async move { crate::tunnel::start_quick_tunnel("app", &state).await })
        };
        poll_until(&state, |s| s.iter().any(|t| t.site == "app")).await;

        crate::tunnel::stop_tunnel("app", None, &state).await;

        let resp = tokio::time::timeout(Duration::from_secs(10), starter)
            .await
            .expect("start did not return after stop")
            .unwrap();
        assert!(matches!(resp, Response::Tunnels { .. }), "{resp:?}");
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_quick_share_that_never_comes_up_is_an_error_and_is_stopped() {
        let tmp = tempfile::tempdir().unwrap();
        let state = stub_state(tmp.path(), FAILING_STUB).await;
        let started = Instant::now();

        let resp = crate::tunnel::start_quick_tunnel("app", &state).await;

        assert!(started.elapsed() < Duration::from_secs(20));
        let Response::Error { message, .. } = resp else {
            panic!("expected an error, got {resp:?}");
        };
        assert!(
            message.contains("failed to request quick Tunnel"),
            "{message}"
        );
        assert!(snapshots(&state).await.is_empty());
        assert!(state.tunnel_routes.read().await.is_empty());
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_quick_share_survives_one_transient_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let state = stub_state(tmp.path(), FLAKY_STUB).await;

        let resp = crate::tunnel::start_quick_tunnel("app", &state).await;

        let Response::Tunnels { tunnels, .. } = resp else {
            panic!("expected Tunnels, got {resp:?}");
        };
        let info = tunnels.iter().find(|t| t.site == "app").unwrap();
        assert_eq!(info.state, TunnelRunState::Running);
        assert_eq!(
            info.url.as_deref(),
            Some("https://stub-tunnel.trycloudflare.com")
        );
        assert!(state.tunnel_routes.read().await.contains_key(STUB_HOST));
        teardown(&state).await;
    }

    const TUNNEL_UUID: &str = "6ff42ae2-765d-4adf-8112-31c55c1551ef";

    async fn named_state(tmp: &std::path::Path) -> Arc<DaemonState> {
        let state = stub_state(tmp, SILENT_STUB).await;
        let blog = Site::linked("blog", tmp.join("blog"), PhpVersion::new(8, 3)).unwrap();
        state.router.write().await.insert(blog).unwrap();
        let cert = super::super::named::origincert(&state.dirs);
        std::fs::create_dir_all(cert.parent().unwrap()).unwrap();
        std::fs::write(&cert, "cert").unwrap();
        state
            .config
            .lock()
            .await
            .tunnel
            .named
            .insert("yerd".to_owned(), TUNNEL_UUID.to_owned());
        state
    }

    async fn expose(state: &Arc<DaemonState>, site: &str, hostname: Option<&str>) {
        let resp = super::super::named::set_site_hostname(site, hostname, state).await;
        assert_eq!(resp, Response::Ok);
    }

    fn named_snapshot(snaps: &[TunnelSnapshot]) -> Option<TunnelSnapshot> {
        snaps.iter().find(|s| s.site == NAMED_KEY).cloned()
    }

    async fn named_running(state: &DaemonState) -> TunnelSnapshot {
        let snaps = poll_until(state, |s| {
            named_snapshot(s).is_some_and(|t| t.state == yerd_tunnel::TunnelState::Running)
        })
        .await;
        named_snapshot(&snaps).unwrap()
    }

    async fn wait_for_mutate_idle(state: &DaemonState) {
        for _ in 0..200 {
            if state.tunnel_mutate.try_lock().is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("tunnel_mutate never became free");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exposing_another_site_never_restarts_the_named_tunnel() {
        let tmp = tempfile::tempdir().unwrap();
        let state = named_state(tmp.path()).await;

        expose(&state, "app", Some("app.example.com")).await;
        let first = named_running(&state).await;
        expose(&state, "blog", Some("blog.example.com")).await;
        wait_for_mutate_idle(&state).await;
        super::super::named::ensure_running(&state).await.unwrap();
        let started = super::super::named::start(&state).await;

        assert!(matches!(started, Response::Tunnels { .. }), "{started:?}");
        let after = named_snapshot(&snapshots(&state).await).unwrap();
        assert_eq!(after.pid, first.pid);
        assert_eq!(after.generation, first.generation);
        let routes = state.tunnel_routes.read().await.clone();
        assert!(routes.contains_key("app.example.com"));
        assert!(routes.contains_key("blog.example.com"));
        assert!(
            std::fs::read_to_string(state.dirs.data.join("tunnel/named.yml"))
                .unwrap()
                .contains(TUNNEL_UUID)
        );
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clearing_the_last_hostname_stops_the_named_tunnel() {
        let tmp = tempfile::tempdir().unwrap();
        let state = named_state(tmp.path()).await;
        expose(&state, "app", Some("app.example.com")).await;
        expose(&state, "blog", Some("blog.example.com")).await;
        let running = named_running(&state).await;
        wait_for_mutate_idle(&state).await;

        expose(&state, "app", None).await;
        let still = named_snapshot(&snapshots(&state).await).unwrap();
        assert_eq!(still.generation, running.generation);

        expose(&state, "blog", None).await;

        assert!(named_snapshot(&snapshots(&state).await).is_none());
        assert!(state.tunnel_routes.read().await.is_empty());
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resume_starts_the_named_tunnel_when_hostnames_are_set() {
        let tmp = tempfile::tempdir().unwrap();
        let state = named_state(tmp.path()).await;
        state
            .config
            .lock()
            .await
            .tunnel
            .sites
            .insert("app".to_owned(), "app.example.com".to_owned());

        resume(Arc::clone(&state), 0).await;

        named_running(&state).await;
        assert!(state
            .tunnel_routes
            .read()
            .await
            .contains_key("app.example.com"));
        assert!(state.share_warnings.lock().await.contains_key("app"));
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resume_with_no_hostnames_starts_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let state = named_state(tmp.path()).await;

        resume(Arc::clone(&state), 0).await;

        assert!(snapshots(&state).await.is_empty());
        teardown(&state).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resume_after_shutdown_spawns_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let state = named_state(tmp.path()).await;
        state
            .config
            .lock()
            .await
            .tunnel
            .sites
            .insert("app".to_owned(), "app.example.com".to_owned());
        state.tunnel_manager.lock().await.shutdown().await.unwrap();

        resume(Arc::clone(&state), 0).await;

        assert!(snapshots(&state).await.is_empty());
        let Err(Response::Error { message, .. }) =
            super::super::named::ensure_running(&state).await
        else {
            panic!("expected a shutdown error");
        };
        assert!(message.contains("shutting down"), "{message}");
    }

    fn warnings_of(resp: &Response) -> Vec<String> {
        let Response::Tunnels { tunnels, .. } = resp else {
            panic!("expected Tunnels, got {resp:?}");
        };
        tunnels
            .iter()
            .find(|t| t.site == "app")
            .unwrap()
            .warnings
            .clone()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn share_warnings_are_cached_at_start_and_not_recomputed_on_status() {
        let tmp = tempfile::tempdir().unwrap();
        let state = stub_state(tmp.path(), QUICK_STUB).await;
        let hot = tmp.path().join("app/hot");
        std::fs::write(&hot, "http://localhost:5173").unwrap();

        let started = crate::tunnel::start_quick_tunnel("app", &state).await;
        assert_eq!(
            warnings_of(&started),
            vec![yerd_tunnel::preflight::VITE_HOT_WARNING.to_owned()]
        );

        std::fs::remove_file(&hot).unwrap();
        let polled = crate::tunnel::tunnel_status(&state).await;
        assert_eq!(warnings_of(&polled), warnings_of(&started));

        let restarted = crate::tunnel::start_quick_tunnel("app", &state).await;
        assert!(warnings_of(&restarted).is_empty());
        teardown(&state).await;
    }
}
