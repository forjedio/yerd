//! `TunnelManager` — drives the shared `yerd-supervise` state machine for one
//! supervised `cloudflared` child per site, doing the real I/O.
//!
//! Mirrors `yerd_services::ServiceManager` in shape (same FSM, same
//! spawn/health/kill driver loop) but differs where a tunnel differs:
//!
//! - **No datadir, no port to bind, no init step** — a tunnel is purely an
//!   outbound child.
//! - **Readiness comes from the logfile, not a socket probe.** The child's
//!   stderr is redirected to a file; readiness is the appearance of the assigned
//!   `*.trycloudflare.com` URL (Quick) or an edge-registration line (Named),
//!   parsed by [`crate::parse`]. The whole (small, startup-phase) logfile is
//!   re-read each tick so a URL line can't scroll past.
//! - **The tunnel [`SupervisorPolicy`]** (generous readiness window, short stop
//!   grace) — `cloudflared` can take seconds to connect and drains cleanly on
//!   SIGTERM.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::time::{Duration, Instant};

use yerd_supervise::supervisor::{
    transition, Action, Elapsed, ErrorTag, Event, KillSignal, PoolState, StopProtocol,
    SupervisorPolicy,
};
use yerd_supervise::{ChildHandle, Clock, ExitReason, ProcessSpawner};

use crate::error::TunnelError;
use crate::parse::{is_named_ready, parse_quick_url};
use crate::TunnelKind;

/// Floor between readiness checks while a tunnel is connecting.
const READINESS_GAP: Duration = Duration::from_millis(150);

/// Live run state of a supervised tunnel, as reported by
/// [`TunnelManager::snapshots`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelState {
    /// The `cloudflared` process is alive and serving.
    Running,
    /// The process has exited unexpectedly.
    Failed,
}

/// A point-in-time view of one supervised tunnel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelSnapshot {
    /// Site name the tunnel publishes.
    pub site: String,
    /// Quick vs Named.
    pub kind: TunnelKind,
    /// Whether the process is alive or has died.
    pub state: TunnelState,
    /// The process PID, when running.
    pub pid: Option<u32>,
    /// The public URL (Quick tunnels) once captured.
    pub url: Option<String>,
    /// The configured public hostname (Named tunnels).
    pub hostname: Option<String>,
}

/// One supervised tunnel instance.
struct Instance<Ch: ChildHandle> {
    state: PoolState,
    state_since: Instant,
    kind: TunnelKind,
    logfile: PathBuf,
    url: Option<String>,
    hostname: Option<String>,
    child: Option<Ch>,
}

/// Outcome of pumping the FSM to a terminal state.
struct RunResult<Ch: ChildHandle> {
    child: Option<Ch>,
    state: PoolState,
    state_since: Instant,
    url: Option<String>,
}

/// Per-drive context that does not change across the loop.
struct DriveCtx<'a> {
    site: &'a str,
    kind: TunnelKind,
    logfile: &'a Path,
}

/// Supervises local `cloudflared` tunnels, one instance per site.
pub struct TunnelManager<S, C>
where
    S: ProcessSpawner,
    C: Clock,
{
    spawner: S,
    clock: C,
    policy: SupervisorPolicy,
    instances: BTreeMap<String, Instance<S::Child>>,
}

impl<S, C> TunnelManager<S, C>
where
    S: ProcessSpawner,
    C: Clock,
{
    /// Construct a manager applying the tunnel [`SupervisorPolicy`] to every
    /// instance.
    pub fn new(spawner: S, clock: C) -> Self {
        Self {
            spawner,
            clock,
            policy: SupervisorPolicy::tunnel(),
            instances: BTreeMap::new(),
        }
    }

    /// Ensure a Quick Tunnel for `site` is running, returning its public
    /// `*.trycloudflare.com` URL. Idempotent: a still-alive tunnel returns its
    /// cached URL.
    pub async fn ensure_quick(
        &mut self,
        site: &str,
        binary: &Path,
        args: Vec<OsString>,
        logfile: PathBuf,
    ) -> Result<String, TunnelError> {
        if let Some(url) = self.running_url(site) {
            return Ok(url);
        }
        let result = self
            .start(site, binary, args, &logfile, TunnelKind::Quick)
            .await?;
        let url = result
            .url
            .clone()
            .ok_or_else(|| TunnelError::ReadinessTimedOut {
                site: site.to_owned(),
            })?;
        self.record(site, TunnelKind::Quick, logfile, result, None);
        Ok(url)
    }

    /// Ensure a Named Tunnel for `site` is running against a prepared config.
    /// Idempotent for an already-running tunnel.
    pub async fn ensure_named(
        &mut self,
        site: &str,
        binary: &Path,
        args: Vec<OsString>,
        logfile: PathBuf,
        hostname: String,
    ) -> Result<(), TunnelError> {
        if self.running_url(site).is_some() || self.is_running(site) {
            return Ok(());
        }
        let result = self
            .start(site, binary, args, &logfile, TunnelKind::Named)
            .await?;
        self.record(site, TunnelKind::Named, logfile, result, Some(hostname));
        Ok(())
    }

    /// Stop the tunnel for `site`. No-op if there is none.
    pub async fn stop(&mut self, site: &str) -> Result<(), TunnelError> {
        let Some(mut inst) = self.instances.remove(site) else {
            return Ok(());
        };
        let child = inst.child.take();
        let ctx = DriveCtx {
            site,
            kind: inst.kind,
            logfile: &inst.logfile,
        };
        self.drive(
            inst.state,
            inst.state_since,
            child,
            Event::StopRequested,
            None,
            &ctx,
        )
        .await
        .map(|_| ())
    }

    /// Stop every supervised tunnel in deterministic order.
    pub async fn shutdown(&mut self) -> Result<(), TunnelError> {
        let sites: Vec<String> = self.instances.keys().cloned().collect();
        let mut first_err: Option<TunnelError> = None;
        for site in sites {
            if let Err(e) = self.stop(&site).await {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    /// Report a live snapshot of every supervised tunnel.
    pub fn snapshots(&mut self) -> Vec<TunnelSnapshot> {
        let mut out = Vec::with_capacity(self.instances.len());
        for (site, inst) in &mut self.instances {
            let (state, pid) = match (&inst.state, inst.child.as_mut()) {
                (PoolState::Running { pid }, Some(child)) => match child.try_wait() {
                    Ok(None) => (TunnelState::Running, Some(*pid)),
                    _ => (TunnelState::Failed, None),
                },
                _ => (TunnelState::Failed, None),
            };
            out.push(TunnelSnapshot {
                site: site.clone(),
                kind: inst.kind,
                state,
                pid,
                url: inst.url.clone(),
                hostname: inst.hostname.clone(),
            });
        }
        out
    }

    /// Fast path: cached public URL of a still-alive Quick tunnel for `site`.
    fn running_url(&mut self, site: &str) -> Option<String> {
        let inst = self.instances.get_mut(site)?;
        if !matches!(inst.state, PoolState::Running { .. }) {
            return None;
        }
        let alive = inst
            .child
            .as_mut()
            .is_some_and(|ch| matches!(ch.try_wait(), Ok(None)));
        alive.then(|| inst.url.clone()).flatten()
    }

    /// Whether `site` has a recorded, still-alive tunnel.
    fn is_running(&mut self, site: &str) -> bool {
        let Some(inst) = self.instances.get_mut(site) else {
            return false;
        };
        matches!(inst.state, PoolState::Running { .. })
            && inst
                .child
                .as_mut()
                .is_some_and(|ch| matches!(ch.try_wait(), Ok(None)))
    }

    /// Spawn and drive a fresh tunnel to `Running`.
    async fn start(
        &mut self,
        site: &str,
        binary: &Path,
        args: Vec<OsString>,
        logfile: &Path,
        kind: TunnelKind,
    ) -> Result<RunResult<S::Child>, TunnelError> {
        if let Some(parent) = logfile.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let cmd_builder = || build_cmd(binary, &args, logfile);
        let ctx = DriveCtx {
            site,
            kind,
            logfile,
        };
        let since = self.clock.now();
        self.drive(
            PoolState::Stopped,
            since,
            None,
            Event::EnsureRequested,
            Some(&cmd_builder),
            &ctx,
        )
        .await
    }

    /// Record a freshly started tunnel in the instance map.
    fn record(
        &mut self,
        site: &str,
        kind: TunnelKind,
        logfile: PathBuf,
        result: RunResult<S::Child>,
        hostname: Option<String>,
    ) {
        self.instances.insert(
            site.to_owned(),
            Instance {
                state: result.state,
                state_since: result.state_since,
                kind,
                logfile,
                url: result.url,
                hostname,
                child: result.child,
            },
        );
    }

    /// Pump the pure state machine to a terminal state, doing the I/O each
    /// `Action` requires. Mirrors `yerd_services::ServiceManager::drive`.
    async fn drive(
        &mut self,
        mut state: PoolState,
        mut state_since: Instant,
        mut child: Option<S::Child>,
        initial: Event,
        cmd_builder: Option<&(dyn Fn() -> Result<StdCommand, TunnelError> + Sync)>,
        ctx: &DriveCtx<'_>,
    ) -> Result<RunResult<S::Child>, TunnelError> {
        let mut pending = initial;
        let mut url: Option<String> = None;
        loop {
            let (next, action) = transition(state, pending, &self.policy);
            if next != state {
                state = next;
                state_since = self.clock.now();
            }

            match action {
                Action::None => {
                    return Ok(RunResult {
                        child,
                        state,
                        state_since,
                        url,
                    });
                }
                Action::Spawn => {
                    pending = self.spawn_child(cmd_builder, &mut child)?;
                }
                Action::HealthCheck => {
                    pending = self
                        .health_check(ctx, state_since, &mut child, &mut url)
                        .await?;
                }
                Action::Backoff { wait } => {
                    tokio::time::sleep(wait).await;
                    pending = Event::BackoffElapsed;
                }
                Action::Kill { signal } => {
                    if let Some(ch) = child.as_mut() {
                        ch.kill(signal, StopProtocol::GroupTerm)
                            .await
                            .map_err(TunnelError::Io)?;
                    }
                    pending =
                        wait_after_kill(&mut child, state, signal, self.policy.stop_grace).await?;
                }
                Action::EmitError(ErrorTag::HealthCheckTimedOut) => {
                    return Err(TunnelError::ReadinessTimedOut {
                        site: ctx.site.to_owned(),
                    });
                }
                Action::EmitError(ErrorTag::PermanentFailure) => {
                    return Err(TunnelError::PermanentFailure {
                        site: ctx.site.to_owned(),
                        last_exit: failed_reason(state),
                    });
                }
            }
        }
    }

    /// Handle `Action::Spawn`: build + spawn the command, record the child.
    fn spawn_child(
        &mut self,
        cmd_builder: Option<&(dyn Fn() -> Result<StdCommand, TunnelError> + Sync)>,
        child: &mut Option<S::Child>,
    ) -> Result<Event, TunnelError> {
        let builder = cmd_builder.ok_or_else(|| {
            TunnelError::Spawn(std::io::Error::other("drive: Spawn without cmd_builder"))
        })?;
        let cmd = builder()?;
        let ch = self.spawner.spawn(cmd).map_err(TunnelError::Spawn)?;
        let pid = ch.id();
        *child = Some(ch);
        Ok(Event::SpawnSucceeded { pid })
    }

    /// Handle `Action::HealthCheck`: read the logfile for the readiness marker,
    /// first checking whether the child already crashed.
    async fn health_check(
        &mut self,
        ctx: &DriveCtx<'_>,
        state_since: Instant,
        child: &mut Option<S::Child>,
        url: &mut Option<String>,
    ) -> Result<Event, TunnelError> {
        tokio::time::sleep(READINESS_GAP).await;
        let ch = child.as_mut().ok_or_else(|| {
            TunnelError::Spawn(std::io::Error::other("HealthCheck with no child handle"))
        })?;
        if let Some(reason) = ch.try_wait().map_err(TunnelError::Io)? {
            *child = None;
            return Ok(Event::Crashed { reason });
        }

        let text = read_file_lossy(ctx.logfile);
        let ready = match ctx.kind {
            TunnelKind::Quick => match parse_quick_url(&text) {
                Some(found) => {
                    *url = Some(found);
                    true
                }
                None => false,
            },
            TunnelKind::Named => is_named_ready(&text),
        };
        if ready {
            Ok(Event::HealthCheckOk)
        } else {
            let elapsed = Elapsed(self.clock.now().saturating_duration_since(state_since));
            Ok(Event::HealthCheckTick {
                elapsed_since_starting: elapsed,
            })
        }
    }
}

/// Build the `cloudflared` command, redirecting stdout+stderr to a freshly
/// truncated logfile (so a stale URL from a prior run is not re-parsed) and, on
/// Unix, putting the child in its own process group so the supervisor's `killpg`
/// reaps it cleanly.
fn build_cmd(binary: &Path, args: &[OsString], logfile: &Path) -> Result<StdCommand, TunnelError> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(logfile)
        .map_err(TunnelError::Io)?;
    let f2 = f.try_clone().map_err(TunnelError::Io)?;
    let mut cmd = StdCommand::new(binary);
    cmd.args(args);
    cmd.stdout(Stdio::from(f2));
    cmd.stderr(Stdio::from(f));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    Ok(cmd)
}

/// Post-kill follow-up: wait for the child to exit (bounded by the grace budget
/// on the first SIGTERM) and return the next synthetic event. Mirrors the
/// `yerd_services` helper of the same name.
async fn wait_after_kill<Ch: ChildHandle>(
    child: &mut Option<Ch>,
    state: PoolState,
    signal: KillSignal,
    stop_grace: Duration,
) -> Result<Event, TunnelError> {
    match (state, signal) {
        (PoolState::Stopping { sigkilled: false }, KillSignal::Term) => {
            let Some(mut owned) = child.take() else {
                return Ok(Event::StopComplete);
            };
            let event = tokio::select! {
                exit = owned.wait() => {
                    exit.map_err(TunnelError::Io)?;
                    Event::StopComplete
                }
                () = tokio::time::sleep(stop_grace) => {
                    *child = Some(owned);
                    return Ok(Event::StopTick { elapsed_since_stopping: Elapsed(stop_grace) });
                }
            };
            Ok(event)
        }
        (PoolState::Stopping { sigkilled: true }, _) => {
            if let Some(ch) = child.as_mut() {
                ch.wait().await.map_err(TunnelError::Io)?;
            }
            *child = None;
            Ok(Event::StopComplete)
        }
        (PoolState::Starting { .. }, KillSignal::Term) => {
            if let Some(ch) = child.as_mut() {
                ch.wait().await.map_err(TunnelError::Io)?;
            }
            *child = None;
            Ok(Event::Crashed {
                reason: ExitReason::Unknown,
            })
        }
        _ => Ok(Event::StopComplete),
    }
}

/// Read a logfile as lossy UTF-8; a missing/unreadable file is treated as empty
/// (the tunnel is simply "not ready yet").
fn read_file_lossy(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => String::new(),
    }
}

fn failed_reason(state: PoolState) -> ExitReason {
    match state {
        PoolState::Failed { last_exit, .. } => last_exit,
        _ => ExitReason::Unknown,
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
    use async_trait::async_trait;
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use yerd_supervise::supervisor::{KillSignal, StopProtocol};

    struct FakeClock;
    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            Instant::now()
        }
    }

    /// A fake child that stays alive until killed, then reports a clean exit.
    struct FakeChild {
        pid: u32,
        killed: Arc<AtomicBool>,
    }

    #[async_trait]
    impl ChildHandle for FakeChild {
        fn id(&self) -> u32 {
            self.pid
        }
        fn try_wait(&mut self) -> Result<Option<ExitReason>, io::Error> {
            if self.killed.load(Ordering::SeqCst) {
                Ok(Some(ExitReason::Code(0)))
            } else {
                Ok(None)
            }
        }
        async fn wait(&mut self) -> Result<ExitReason, io::Error> {
            Ok(ExitReason::Code(0))
        }
        async fn kill(
            &mut self,
            _signal: KillSignal,
            _protocol: StopProtocol,
        ) -> Result<(), io::Error> {
            self.killed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Writes a canned logfile when spawned (standing in for cloudflared's own
    /// output), then returns a live [`FakeChild`].
    struct FakeSpawner {
        logfile: PathBuf,
        contents: String,
        killed: Arc<AtomicBool>,
    }

    impl ProcessSpawner for FakeSpawner {
        type Child = FakeChild;
        fn spawn(&self, _cmd: StdCommand) -> Result<FakeChild, io::Error> {
            std::fs::write(&self.logfile, self.contents.as_bytes())?;
            Ok(FakeChild {
                pid: 4242,
                killed: Arc::clone(&self.killed),
            })
        }
    }

    #[tokio::test]
    async fn ensure_quick_captures_url_and_reports_running() {
        let tmp = tempfile::tempdir().unwrap();
        let logfile = tmp.path().join("app.log");
        let killed = Arc::new(AtomicBool::new(false));
        let spawner = FakeSpawner {
            logfile: logfile.clone(),
            contents: "INF |  https://calm-river-1234.trycloudflare.com  |\n".to_string(),
            killed: Arc::clone(&killed),
        };
        let mut mgr = TunnelManager::new(spawner, FakeClock);

        let url = mgr
            .ensure_quick("app", Path::new("/usr/bin/cloudflared"), vec![], logfile)
            .await
            .unwrap();
        assert_eq!(url, "https://calm-river-1234.trycloudflare.com");

        let snaps = mgr.snapshots();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].site, "app");
        assert_eq!(snaps[0].kind, TunnelKind::Quick);
        assert_eq!(snaps[0].state, TunnelState::Running);
        assert_eq!(snaps[0].url.as_deref(), Some(url.as_str()));

        // Idempotent: a second ensure returns the cached URL without respawning.
        let again = mgr
            .ensure_quick(
                "app",
                Path::new("/usr/bin/cloudflared"),
                vec![],
                tmp.path().join("app.log"),
            )
            .await
            .unwrap();
        assert_eq!(again, "https://calm-river-1234.trycloudflare.com");
    }

    #[tokio::test]
    async fn stop_removes_the_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let logfile = tmp.path().join("blog.log");
        let killed = Arc::new(AtomicBool::new(false));
        let spawner = FakeSpawner {
            logfile: logfile.clone(),
            contents: "https://a-b-c.trycloudflare.com\n".to_string(),
            killed,
        };
        let mut mgr = TunnelManager::new(spawner, FakeClock);
        mgr.ensure_quick("blog", Path::new("/cf"), vec![], logfile)
            .await
            .unwrap();
        assert_eq!(mgr.snapshots().len(), 1);
        mgr.stop("blog").await.unwrap();
        assert!(mgr.snapshots().is_empty());
        // Stopping an unknown site is a no-op.
        mgr.stop("nope").await.unwrap();
    }

    #[tokio::test]
    async fn ensure_named_marks_running_on_registration_line() {
        let tmp = tempfile::tempdir().unwrap();
        let logfile = tmp.path().join("named.log");
        let killed = Arc::new(AtomicBool::new(false));
        let spawner = FakeSpawner {
            logfile: logfile.clone(),
            contents: "INF Registered tunnel connection connIndex=0\n".to_string(),
            killed,
        };
        let mut mgr = TunnelManager::new(spawner, FakeClock);
        mgr.ensure_named(
            "shop",
            Path::new("/cf"),
            vec![],
            logfile,
            "shop.example.com".to_string(),
        )
        .await
        .unwrap();
        let snaps = mgr.snapshots();
        assert_eq!(snaps[0].kind, TunnelKind::Named);
        assert_eq!(snaps[0].state, TunnelState::Running);
        assert_eq!(snaps[0].hostname.as_deref(), Some("shop.example.com"));
    }
}
