//! `TunnelManager` drives the shared `yerd-supervise` state machine for one
//! supervised `cloudflared` child per site, doing the real I/O.
//!
//! Differs from `yerd_services::ServiceManager` in one important way: a tunnel's
//! readiness can take tens of seconds (a cold edge connect), and the whole
//! manager sits behind a single async mutex shared by every tunnel op. So rather
//! than pump the FSM to a terminal state inside one `&mut self` call (which would
//! hold that mutex for the entire multi-minute readiness drive and make a stuck
//! start un-stoppable), supervision is **tick-based**: [`begin`](TunnelManager::begin)
//! spawns the child and the caller then calls [`advance`](TunnelManager::advance)
//! repeatedly, **re-acquiring the lock per tick and sleeping with the lock
//! released**. Every lock-hold is therefore bounded (a sync FSM step, or at most
//! one stop-grace window on a kill), so `StopTunnel`/`TunnelStatus`/shutdown stay
//! responsive and a stuck start can be cancelled.
//!
//! Other differences from a database service: no datadir/port/init, and
//! readiness comes from the logfile (the child's stderr is redirected there):
//! the assigned `*.trycloudflare.com` URL (Quick) or an edge-registration line
//! (Named), parsed by [`crate::parse`].

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

/// What the caller should do after one [`TunnelManager::advance`] tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Call `advance` again immediately (the FSM took a non-waiting action).
    Continue,
    /// Sleep this long (lock released), then call `advance` again.
    Sleep(Duration),
    /// The tunnel is up; its URL (Quick) is captured. Read it via `snapshots`.
    Ready,
    /// The instance no longer exists (it was stopped concurrently). Stop driving.
    Gone,
}

/// Live run state of a supervised tunnel, as reported by
/// [`TunnelManager::snapshots`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelState {
    /// The `cloudflared` process is alive (connecting or serving).
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

/// One supervised tunnel instance. The child lives here for its whole life (not
/// owned by a transient driver), so a concurrent `stop` can reach it.
struct Instance<Ch: ChildHandle> {
    state: PoolState,
    state_since: Instant,
    /// The event fed to the FSM on the next `advance` tick.
    pending: Event,
    kind: TunnelKind,
    binary: PathBuf,
    args: Vec<OsString>,
    logfile: PathBuf,
    url: Option<String>,
    hostname: Option<String>,
    child: Option<Ch>,
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

    /// Begin supervising a tunnel for `site`: register it and spawn the child
    /// (the first FSM step), leaving it `Starting`. Returns `Ok(false)` without
    /// touching anything if a tunnel for `site` is already live or starting
    /// (idempotent / concurrent-start safe), `Ok(true)` if a fresh one was
    /// started. The caller then drives readiness via [`Self::advance`].
    ///
    /// The first step (the spawn) is primed synchronously here so the instance
    /// is immediately `Starting` with a child, closing the window in which a
    /// concurrent `begin` could see a half-registered `Stopped` instance.
    pub async fn begin(
        &mut self,
        site: &str,
        binary: &Path,
        args: Vec<OsString>,
        logfile: PathBuf,
        kind: TunnelKind,
        hostname: Option<String>,
    ) -> Result<bool, TunnelError> {
        if let Some(inst) = self.instances.get_mut(site) {
            let alive = inst
                .child
                .as_mut()
                .is_some_and(|ch| matches!(ch.try_wait(), Ok(None)));
            let in_progress = matches!(
                inst.state,
                PoolState::Starting { .. } | PoolState::Stopping { .. }
            );
            if alive || in_progress {
                return Ok(false);
            }
        }
        if let Some(parent) = logfile.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let now = self.clock.now();
        self.instances.insert(
            site.to_owned(),
            Instance {
                state: PoolState::Stopped,
                state_since: now,
                pending: Event::EnsureRequested,
                kind,
                binary: binary.to_path_buf(),
                args,
                logfile,
                url: None,
                hostname,
                child: None,
            },
        );
        if let Err(e) = self.advance(site).await {
            self.instances.remove(site);
            return Err(e);
        }
        Ok(true)
    }

    /// Drive the FSM one step for `site`, doing the I/O its action requires.
    /// Every step is bounded: a synchronous FSM transition, or at most one
    /// stop-grace window when a readiness timeout has to kill the child.
    pub async fn advance(&mut self, site: &str) -> Result<Step, TunnelError> {
        let policy = self.policy;
        let now = self.clock.now();
        let Some((state, pending, state_since)) = self
            .instances
            .get(site)
            .map(|i| (i.state, i.pending, i.state_since))
        else {
            return Ok(Step::Gone);
        };

        let (next, action) = transition(state, pending, &policy);
        if next != state {
            if let Some(i) = self.instances.get_mut(site) {
                i.state = next;
                i.state_since = now;
            }
        }

        match action {
            Action::None => match next {
                PoolState::Running { .. } => Ok(Step::Ready),
                PoolState::Stopped => Ok(Step::Gone),
                other => Err(TunnelError::Spawn(std::io::Error::other(format!(
                    "advance: Action::None in non-terminal state {other:?}"
                )))),
            },
            Action::Spawn => self.do_spawn(site),
            Action::HealthCheck => Ok(self.do_health_check(site, state_since, now)),
            Action::Backoff { wait } => {
                self.set_pending(site, Event::BackoffElapsed);
                Ok(Step::Sleep(wait))
            }
            Action::Kill { signal } => self.do_kill(site, signal).await,
            Action::EmitError(ErrorTag::HealthCheckTimedOut) => {
                Err(TunnelError::ReadinessTimedOut {
                    site: site.to_owned(),
                })
            }
            Action::EmitError(ErrorTag::PermanentFailure) => Err(TunnelError::PermanentFailure {
                site: site.to_owned(),
                last_exit: failed_reason(next),
            }),
        }
    }

    /// `Action::Spawn`: build + spawn the child, record it, advance to readiness.
    fn do_spawn(&mut self, site: &str) -> Result<Step, TunnelError> {
        let Some((binary, args, logfile)) = self
            .instances
            .get(site)
            .map(|i| (i.binary.clone(), i.args.clone(), i.logfile.clone()))
        else {
            return Ok(Step::Gone);
        };
        let cmd = build_cmd(&binary, &args, &logfile)?;
        let child = self.spawner.spawn(cmd).map_err(TunnelError::Spawn)?;
        let pid = child.id();
        if let Some(i) = self.instances.get_mut(site) {
            i.child = Some(child);
            i.pending = Event::SpawnSucceeded { pid };
        }
        Ok(Step::Continue)
    }

    /// `Action::HealthCheck`: non-blocking. Check for a crash, then read the
    /// logfile for the readiness marker. Never `await`s, so the lock-hold is a
    /// file read.
    fn do_health_check(&mut self, site: &str, state_since: Instant, now: Instant) -> Step {
        let crashed = self
            .instances
            .get_mut(site)
            .and_then(|i| i.child.as_mut())
            .and_then(|ch| ch.try_wait().ok().flatten());
        if let Some(reason) = crashed {
            if let Some(i) = self.instances.get_mut(site) {
                i.child = None;
                i.pending = Event::Crashed { reason };
            }
            return Step::Continue;
        }

        let Some((kind, logfile)) = self
            .instances
            .get(site)
            .map(|i| (i.kind, i.logfile.clone()))
        else {
            return Step::Gone;
        };
        let text = read_file_lossy(&logfile);
        let ready = match kind {
            TunnelKind::Quick => match parse_quick_url(&text) {
                Some(url) => {
                    if let Some(i) = self.instances.get_mut(site) {
                        i.url = Some(url);
                    }
                    true
                }
                None => false,
            },
            TunnelKind::Named => is_named_ready(&text),
        };
        if ready {
            self.set_pending(site, Event::HealthCheckOk);
            Step::Continue
        } else {
            let elapsed = Elapsed(now.saturating_duration_since(state_since));
            self.set_pending(
                site,
                Event::HealthCheckTick {
                    elapsed_since_starting: elapsed,
                },
            );
            Step::Sleep(READINESS_GAP)
        }
    }

    /// `Action::Kill` (only reached on a readiness timeout): SIGTERM the child,
    /// wait up to the stop grace, then SIGKILL (bounded), and feed `Crashed` so
    /// the FSM applies its restart/backoff policy.
    async fn do_kill(&mut self, site: &str, signal: KillSignal) -> Result<Step, TunnelError> {
        let mut child = self.instances.get_mut(site).and_then(|i| i.child.take());
        if let Some(ch) = child.as_mut() {
            ch.kill(signal, StopProtocol::GroupTerm)
                .await
                .map_err(TunnelError::Io)?;
            graceful_reap(ch, self.policy.stop_grace).await;
        }
        if let Some(i) = self.instances.get_mut(site) {
            i.child = None;
            i.pending = Event::Crashed {
                reason: ExitReason::Unknown,
            };
        }
        Ok(Step::Continue)
    }

    fn set_pending(&mut self, site: &str, event: Event) {
        if let Some(i) = self.instances.get_mut(site) {
            i.pending = event;
        }
    }

    /// Stop the tunnel for `site`: SIGTERM, wait up to the stop grace, then
    /// SIGKILL, and drop the instance. No-op if there is none. Bounded by the
    /// stop grace.
    pub async fn stop(&mut self, site: &str) -> Result<(), TunnelError> {
        let Some(mut inst) = self.instances.remove(site) else {
            return Ok(());
        };
        if let Some(mut child) = inst.child.take() {
            child
                .kill(KillSignal::Term, StopProtocol::GroupTerm)
                .await
                .map_err(TunnelError::Io)?;
            graceful_reap(&mut child, self.policy.stop_grace).await;
        }
        Ok(())
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

    /// Report a live snapshot of every supervised tunnel. A still-connecting
    /// (`Starting`) tunnel with a live child reads as `Running` with no URL yet.
    pub fn snapshots(&mut self) -> Vec<TunnelSnapshot> {
        let mut out = Vec::with_capacity(self.instances.len());
        for (site, inst) in &mut self.instances {
            let alive = inst
                .child
                .as_mut()
                .is_some_and(|ch| matches!(ch.try_wait(), Ok(None)));
            let running = alive
                && matches!(
                    inst.state,
                    PoolState::Running { .. } | PoolState::Starting { .. }
                );
            let pid = if alive {
                inst.child.as_ref().map(ChildHandle::id)
            } else {
                None
            };
            out.push(TunnelSnapshot {
                site: site.clone(),
                kind: inst.kind,
                state: if running {
                    TunnelState::Running
                } else {
                    TunnelState::Failed
                },
                pid,
                url: inst.url.clone(),
                hostname: inst.hostname.clone(),
            });
        }
        out
    }
}

/// Reap a child that has been signalled: wait up to `grace`, then SIGKILL and
/// wait. Errors are swallowed (teardown is best-effort).
async fn graceful_reap<Ch: ChildHandle>(child: &mut Ch, grace: Duration) {
    tokio::select! {
        _ = child.wait() => {}
        () = tokio::time::sleep(grace) => {
            let _ = child.kill(KillSignal::Kill, StopProtocol::GroupTerm).await;
            let _ = child.wait().await;
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

    /// Drive a freshly-begun tunnel to readiness the way the daemon handler does.
    async fn drive_to_ready<S: ProcessSpawner, C: Clock>(
        mgr: &mut TunnelManager<S, C>,
        site: &str,
    ) -> Step {
        loop {
            match mgr.advance(site).await.unwrap() {
                Step::Continue | Step::Sleep(_) => {}
                done => return done,
            }
        }
    }

    #[tokio::test]
    async fn begin_then_advance_captures_url_and_reports_running() {
        let tmp = tempfile::tempdir().unwrap();
        let logfile = tmp.path().join("app.log");
        let killed = Arc::new(AtomicBool::new(false));
        let spawner = FakeSpawner {
            logfile: logfile.clone(),
            contents: "INF |  https://calm-river-1234.trycloudflare.com  |\n".to_string(),
            killed: Arc::clone(&killed),
        };
        let mut mgr = TunnelManager::new(spawner, FakeClock);

        let started = mgr
            .begin(
                "app",
                Path::new("/usr/bin/cloudflared"),
                vec![],
                logfile,
                TunnelKind::Quick,
                None,
            )
            .await
            .unwrap();
        assert!(started);

        assert_eq!(drive_to_ready(&mut mgr, "app").await, Step::Ready);

        let snaps = mgr.snapshots();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].site, "app");
        assert_eq!(snaps[0].kind, TunnelKind::Quick);
        assert_eq!(snaps[0].state, TunnelState::Running);
        assert_eq!(
            snaps[0].url.as_deref(),
            Some("https://calm-river-1234.trycloudflare.com")
        );

        // Idempotent: a second begin for a live site does not restart it.
        let again = mgr
            .begin(
                "app",
                Path::new("/usr/bin/cloudflared"),
                vec![],
                tmp.path().join("app.log"),
                TunnelKind::Quick,
                None,
            )
            .await
            .unwrap();
        assert!(!again);
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
        mgr.begin(
            "blog",
            Path::new("/cf"),
            vec![],
            logfile,
            TunnelKind::Quick,
            None,
        )
        .await
        .unwrap();
        assert_eq!(drive_to_ready(&mut mgr, "blog").await, Step::Ready);
        assert_eq!(mgr.snapshots().len(), 1);
        mgr.stop("blog").await.unwrap();
        assert!(mgr.snapshots().is_empty());
        // Stopping an unknown site is a no-op; advancing a gone site is Gone.
        mgr.stop("nope").await.unwrap();
        assert_eq!(mgr.advance("blog").await.unwrap(), Step::Gone);
    }

    #[tokio::test]
    async fn named_tunnel_marks_running_on_registration_line() {
        let tmp = tempfile::tempdir().unwrap();
        let logfile = tmp.path().join("named.log");
        let killed = Arc::new(AtomicBool::new(false));
        let spawner = FakeSpawner {
            logfile: logfile.clone(),
            contents: "INF Registered tunnel connection connIndex=0\n".to_string(),
            killed,
        };
        let mut mgr = TunnelManager::new(spawner, FakeClock);
        mgr.begin(
            "shop",
            Path::new("/cf"),
            vec![],
            logfile,
            TunnelKind::Named,
            Some("shop.example.com".to_string()),
        )
        .await
        .unwrap();
        assert_eq!(drive_to_ready(&mut mgr, "shop").await, Step::Ready);
        let snaps = mgr.snapshots();
        assert_eq!(snaps[0].kind, TunnelKind::Named);
        assert_eq!(snaps[0].state, TunnelState::Running);
        assert_eq!(snaps[0].hostname.as_deref(), Some("shop.example.com"));
    }
}
