//! `TunnelManager` drives the shared `yerd-supervise` state machine for one
//! supervised `cloudflared` child per key, doing the real I/O.
//!
//! Differs from `yerd_services::ServiceManager` in one important way: a tunnel's
//! readiness can take tens of seconds (a cold edge connect), and the whole
//! manager sits behind a single async mutex shared by every tunnel op. So rather
//! than pump the FSM inside one `&mut self` call, supervision is **tick-based**:
//! [`begin`](TunnelManager::begin) spawns the child and a driver task then calls
//! [`advance`](TunnelManager::advance) repeatedly, **re-acquiring the lock per
//! tick and sleeping with the lock released**. Every lock-hold is bounded (a sync
//! FSM step plus a log read, or at most one stop-grace window on a kill), so
//! `StopTunnel`/`TunnelStatus`/shutdown stay responsive.
//!
//! Supervision is continuous: once `Running`, every tick checks the child is
//! still alive and asks the driver to probe `cloudflared`'s `/ready` endpoint.
//! The probe never runs under the lock: `advance` returns [`Step::Probe`] with
//! the metrics address, the driver probes with the lock released, and then feeds
//! the result back through [`record_probe`](TunnelManager::record_probe). A crash
//! goes through the FSM's backoff and is respawned without limit (see
//! `SupervisorPolicy::tunnel`).
//!
//! Each fresh [`begin`](TunnelManager::begin) draws a new, manager-wide
//! **generation**. `advance` and `record_probe` take the generation their driver
//! was started with and report [`Step::Gone`] for any other, so a stale driver
//! left over from a stop and re-start can never act on its replacement.
//!
//! The metrics address is picked fresh on every spawn (through
//! [`LaunchSpec::metrics_addr`]) and handed to the child as `TUNNEL_METRICS`, so
//! a port taken by another process costs one failed attempt, not a crash loop.
//! A pidfile is written per spawn and removed on stop, so the daemon can reap a
//! `cloudflared` orphaned by a daemon crash.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use yerd_supervise::supervisor::{
    transition, Action, Elapsed, ErrorTag, Event, KillSignal, PoolState, StopProtocol,
    SupervisorPolicy,
};
use yerd_supervise::{ChildHandle, Clock, ExitReason, ProcessSpawner};

use crate::error::TunnelError;
use crate::parse::{last_error_line, parse_quick_url};
use crate::TunnelKind;

/// Floor between readiness checks while a tunnel is connecting.
const READINESS_GAP: Duration = Duration::from_millis(150);

/// Interval between liveness and `/ready` checks once a tunnel is running.
pub const MONITOR_INTERVAL: Duration = Duration::from_secs(2);

/// The env var `cloudflared` reads its metrics server address from.
const METRICS_ENV: &str = "TUNNEL_METRICS";

/// Source of a fresh loopback metrics address, called once per spawn.
pub type MetricsAddrSource = Arc<dyn Fn() -> std::io::Result<SocketAddr> + Send + Sync>;

/// Everything needed to spawn one supervised `cloudflared` child: the binary,
/// its argument vector, the pinned environment, the logfile its output is
/// redirected to, the per-spawn metrics address source, and its pidfile. Passed
/// to [`TunnelManager::begin`].
#[derive(Clone)]
pub struct LaunchSpec {
    /// Path to the `cloudflared` binary.
    pub binary: PathBuf,
    /// Argument vector (excluding the program name).
    pub args: Vec<OsString>,
    /// The child's complete environment: the inherited environment is cleared
    /// before these are applied, so this is the only environment `cloudflared`
    /// sees (e.g. `HOME`, `TUNNEL_ORIGIN_CERT`, and any forwarded allowlist).
    /// `TUNNEL_METRICS` is appended per spawn.
    pub env: Vec<(OsString, OsString)>,
    /// Logfile the child's stdout+stderr are redirected to (and tailed for
    /// readiness and errors).
    pub logfile: PathBuf,
    /// Called on every spawn for the metrics address passed as `TUNNEL_METRICS`
    /// and probed for `/ready`.
    pub metrics_addr: MetricsAddrSource,
    /// Where the live child's PID is recorded (written per spawn, removed on
    /// stop) so a daemon crash's orphan can be reaped on the next start.
    pub pidfile: PathBuf,
}

impl fmt::Debug for LaunchSpec {
    /// Prints the paths and args only: env values may carry secrets, and the
    /// metrics source is a closure.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaunchSpec")
            .field("binary", &self.binary)
            .field("args", &self.args)
            .field("logfile", &self.logfile)
            .field("pidfile", &self.pidfile)
            .finish_non_exhaustive()
    }
}

/// What the caller should do after one [`TunnelManager::advance`] tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Call `advance` again immediately (the FSM took a non-waiting action).
    Continue,
    /// Sleep this long (lock released), then call `advance` again.
    Sleep(Duration),
    /// Probe `cloudflared`'s `/ready` at this address with the lock released,
    /// then report the outcome through [`TunnelManager::record_probe`].
    Probe(SocketAddr),
    /// The tunnel just came up (returned once per `Starting` to `Running`
    /// transition); a Quick tunnel's URL is captured. Read it via `snapshots`.
    Ready,
    /// The instance no longer exists, or belongs to a newer generation. Stop
    /// driving.
    Gone,
}

/// Live run state of a supervised tunnel, as reported by
/// [`TunnelManager::snapshots`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelState {
    /// Starting for the first time; the edge has never been reached.
    Connecting,
    /// Running with a healthy edge connection.
    Running,
    /// Was online before and is now restarting, or is alive with the edge down.
    Reconnecting,
    /// The process exited; a restart is pending after backoff.
    Failed,
}

/// A point-in-time view of one supervised tunnel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelSnapshot {
    /// Supervisor key: the site name, or a reserved key for the named tunnel.
    pub site: String,
    /// Quick vs Named.
    pub kind: TunnelKind,
    /// Reported health (see [`TunnelState`]).
    pub state: TunnelState,
    /// The process PID, when alive.
    pub pid: Option<u32>,
    /// The public URL (Quick tunnels) once captured for the current spawn.
    pub url: Option<String>,
    /// The generation this instance was begun with.
    pub generation: u64,
    /// Why the most recent attempt failed, cleared once the edge is reached.
    pub last_error: Option<String>,
    /// Consecutive failed attempts while in [`TunnelState::Failed`] (reset once
    /// the instance comes online), `0` in any other state.
    pub failures: u32,
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
    /// The child's complete environment, applied after the inherited
    /// environment is cleared on every spawn (e.g. `HOME`, `TUNNEL_ORIGIN_CERT`,
    /// and any forwarded allowlist), so `cloudflared` never resolves against the
    /// real user's home or inherits unrelated daemon secrets.
    env: Vec<(OsString, OsString)>,
    logfile: PathBuf,
    metrics_addr: MetricsAddrSource,
    pidfile: PathBuf,
    url: Option<String>,
    /// Set when the child was killed for missing its readiness window, and
    /// cleared on each respawn so it reflects only the most recent attempt.
    timed_out: bool,
    child: Option<Ch>,
    generation: u64,
    /// Whether this instance ever reached `Running`.
    ever_ready: bool,
    /// The last `/ready` outcome for the current spawn.
    edge_up: bool,
    last_error: Option<String>,
    /// The metrics address handed to the current spawn.
    metrics: Option<SocketAddr>,
}

/// Supervises local `cloudflared` tunnels, one instance per key.
pub struct TunnelManager<S, C>
where
    S: ProcessSpawner,
    C: Clock,
{
    spawner: S,
    clock: C,
    policy: SupervisorPolicy,
    instances: BTreeMap<String, Instance<S::Child>>,
    /// Last generation handed out; monotonic across every key.
    next_generation: u64,
    /// Set by [`Self::shutdown`]; refuses every later `begin`.
    closed: bool,
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
            next_generation: 0,
            closed: false,
        }
    }

    /// Begin supervising a tunnel under `site`: register it and spawn the child
    /// (the first FSM step), leaving it `Starting`. Returns `Ok(Some(generation))`
    /// for a fresh start, which the caller passes to every [`Self::advance`] of
    /// its driver. Returns `Ok(None)` without touching anything when an instance
    /// is live or in progress, which includes `Failed` with a restart pending, so
    /// a repeat start never restarts a working connector and keeps a retrying
    /// one's history. The exception is a `Failed` instance whose stored command
    /// (binary, args, env) differs from `spec`: it is retrying something stale,
    /// say a `cloudflared` that has since been removed, so it is replaced.
    ///
    /// An indeterminate `try_wait` (an `Err`, never `Ok`) counts the existing
    /// child as alive, so a transient wait error can never make `begin`
    /// overwrite a still-running instance and spawn a duplicate `cloudflared`.
    ///
    /// # Errors
    ///
    /// [`TunnelError::ShuttingDown`] once [`Self::shutdown`] has run.
    pub async fn begin(
        &mut self,
        site: &str,
        spec: LaunchSpec,
        kind: TunnelKind,
    ) -> Result<Option<u64>, TunnelError> {
        if self.closed {
            return Err(TunnelError::ShuttingDown);
        }
        let policy = self.policy;
        if let Some(inst) = self.instances.get_mut(site) {
            let alive = inst
                .child
                .as_mut()
                .is_some_and(|ch| !matches!(ch.try_wait(), Ok(Some(_))));
            let in_progress = match inst.state {
                PoolState::Starting { .. }
                | PoolState::Running { .. }
                | PoolState::Stopping { .. } => true,
                PoolState::Failed { attempts, .. } => {
                    attempts < policy.max_restart_attempts && same_command(inst, &spec)
                }
                PoolState::Stopped => false,
            };
            if alive || in_progress {
                return Ok(None);
            }
        }
        if let Some(parent) = spec.logfile.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        self.next_generation = self.next_generation.saturating_add(1);
        let generation = self.next_generation;
        let now = self.clock.now();
        self.instances.insert(
            site.to_owned(),
            Instance {
                state: PoolState::Stopped,
                state_since: now,
                pending: Event::EnsureRequested,
                kind,
                binary: spec.binary,
                args: spec.args,
                env: spec.env,
                logfile: spec.logfile,
                metrics_addr: spec.metrics_addr,
                pidfile: spec.pidfile,
                url: None,
                timed_out: false,
                child: None,
                generation,
                ever_ready: false,
                edge_up: false,
                last_error: None,
                metrics: None,
            },
        );
        if let Err(e) = self.advance(site, generation).await {
            self.instances.remove(site);
            return Err(e);
        }
        Ok(Some(generation))
    }

    /// Drive the FSM one step for `site`'s `generation`, doing the I/O its
    /// action requires. Returns [`Step::Gone`] when the instance is missing or
    /// has a different generation. Every step is bounded: a synchronous FSM
    /// transition plus a log read, or at most one stop-grace window when a
    /// readiness timeout has to kill the child. A running instance is checked
    /// for an exit and then handed back as [`Step::Probe`].
    ///
    /// # Errors
    ///
    /// Signalling a child on a readiness timeout failed, or the FSM reported a
    /// terminal failure (unreachable under the never-give-up tunnel policy).
    pub async fn advance(&mut self, site: &str, generation: u64) -> Result<Step, TunnelError> {
        let policy = self.policy;
        let now = self.clock.now();
        let Some((state, pending, state_since)) = self
            .instances
            .get(site)
            .filter(|i| i.generation == generation)
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
            Action::None => match (state, next) {
                (PoolState::Starting { .. }, PoolState::Running { .. }) => Ok(Step::Ready),
                (_, PoolState::Running { .. }) => Ok(self.monitor(site)),
                (_, PoolState::Stopped) => Ok(Step::Gone),
                (_, other) => Err(TunnelError::Spawn(std::io::Error::other(format!(
                    "advance: Action::None in non-terminal state {other:?}"
                )))),
            },
            Action::Spawn => Ok(self.do_spawn(site)),
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
            Action::EmitError(ErrorTag::PermanentFailure)
                if self.instances.get(site).is_some_and(|i| i.timed_out) =>
            {
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

    /// Fold a `/ready` probe outcome for `site`'s `generation` into the FSM and
    /// return what the driver does next. A missing instance or a stale
    /// generation discards the result and returns [`Step::Gone`].
    ///
    /// While `Starting`, an OK probe (plus the URL, for a Quick tunnel) brings the
    /// tunnel up and clears `last_error`; anything else counts toward the
    /// readiness window. While `Running`, it only records edge health, so an
    /// edge outage reads as `Reconnecting` without a respawn (`cloudflared`
    /// reconnects by itself).
    pub fn record_probe(&mut self, site: &str, generation: u64, ok: bool) -> Step {
        let now = self.clock.now();
        let Some(inst) = self
            .instances
            .get_mut(site)
            .filter(|i| i.generation == generation)
        else {
            return Step::Gone;
        };
        match inst.state {
            PoolState::Starting { .. } => {
                let ready = ok && (inst.kind == TunnelKind::Named || inst.url.is_some());
                if ready {
                    inst.pending = Event::HealthCheckOk;
                    inst.ever_ready = true;
                    inst.edge_up = true;
                    inst.last_error = None;
                    Step::Continue
                } else {
                    inst.pending = Event::HealthCheckTick {
                        elapsed_since_starting: Elapsed(
                            now.saturating_duration_since(inst.state_since),
                        ),
                    };
                    Step::Sleep(READINESS_GAP)
                }
            }
            PoolState::Running { .. } => {
                inst.edge_up = ok;
                Step::Sleep(MONITOR_INTERVAL)
            }
            PoolState::Stopped | PoolState::Failed { .. } | PoolState::Stopping { .. } => {
                Step::Continue
            }
        }
    }

    /// The `Running` monitor: an exited child feeds `Crashed` (so the next tick
    /// backs off and respawns); a live one is handed back for a `/ready` probe.
    fn monitor(&mut self, site: &str) -> Step {
        let Some(inst) = self.instances.get_mut(site) else {
            return Step::Gone;
        };
        if let Some(reason) = child_exit(inst.child.as_mut()) {
            record_crash(inst, reason);
            return Step::Continue;
        }
        inst.metrics
            .map_or(Step::Sleep(MONITOR_INTERVAL), Step::Probe)
    }

    /// `Action::Spawn`: pick this spawn's metrics address, build + spawn the
    /// child, and record it with its pidfile. A failure at any point is recorded
    /// as `last_error` and fed to the FSM as a crash, so it backs off and retries
    /// instead of ending supervision.
    fn do_spawn(&mut self, site: &str) -> Step {
        let Some((binary, args, env, logfile, metrics_addr)) = self.instances.get(site).map(|i| {
            (
                i.binary.clone(),
                i.args.clone(),
                i.env.clone(),
                i.logfile.clone(),
                Arc::clone(&i.metrics_addr),
            )
        }) else {
            return Step::Gone;
        };
        let spawned = self.spawn_child(&binary, &args, env, &logfile, &*metrics_addr);
        let Some(inst) = self.instances.get_mut(site) else {
            return Step::Gone;
        };
        inst.url = None;
        inst.edge_up = false;
        inst.timed_out = false;
        match spawned {
            Ok((addr, child)) => {
                let pid = child.id();
                write_pidfile(&inst.pidfile, pid);
                inst.metrics = Some(addr);
                inst.child = Some(child);
                inst.pending = Event::SpawnSucceeded { pid };
            }
            Err(message) => {
                inst.metrics = None;
                inst.child = None;
                inst.last_error = Some(message);
                inst.pending = Event::Crashed {
                    reason: ExitReason::Unknown,
                };
            }
        }
        Step::Continue
    }

    fn spawn_child(
        &self,
        binary: &Path,
        args: &[OsString],
        mut env: Vec<(OsString, OsString)>,
        logfile: &Path,
        metrics_addr: &(dyn Fn() -> std::io::Result<SocketAddr> + Send + Sync),
    ) -> Result<(SocketAddr, S::Child), String> {
        let addr = metrics_addr().map_err(|e| format!("could not pick a metrics address: {e}"))?;
        env.push((METRICS_ENV.into(), addr.to_string().into()));
        let cmd = build_cmd(binary, args, &env, logfile).map_err(|e| e.to_string())?;
        let child = self
            .spawner
            .spawn(cmd)
            .map_err(|e| TunnelError::Spawn(e).to_string())?;
        Ok((addr, child))
    }

    /// `Action::HealthCheck` (while `Starting`): non-blocking. Check for a
    /// crash, capture a Quick tunnel's URL from the log, then hand back a probe
    /// once there is something to confirm. Never `await`s, so the lock-hold is
    /// a file read.
    fn do_health_check(&mut self, site: &str, state_since: Instant, now: Instant) -> Step {
        let Some(inst) = self.instances.get_mut(site) else {
            return Step::Gone;
        };
        if let Some(reason) = child_exit(inst.child.as_mut()) {
            record_crash(inst, reason);
            return Step::Continue;
        }
        if inst.kind == TunnelKind::Quick && inst.url.is_none() {
            inst.url = parse_quick_url(&read_file_lossy(&inst.logfile));
        }
        let can_be_ready = inst.kind == TunnelKind::Named || inst.url.is_some();
        if let (true, Some(addr)) = (can_be_ready, inst.metrics) {
            return Step::Probe(addr);
        }
        inst.pending = Event::HealthCheckTick {
            elapsed_since_starting: Elapsed(now.saturating_duration_since(state_since)),
        };
        Step::Sleep(READINESS_GAP)
    }

    /// `Action::Kill` (only reached on a readiness timeout): SIGTERM the child,
    /// wait up to the stop grace, then SIGKILL (bounded), record why, and feed
    /// `Crashed` so the FSM applies its restart/backoff policy.
    async fn do_kill(&mut self, site: &str, signal: KillSignal) -> Result<Step, TunnelError> {
        let mut child = self.instances.get_mut(site).and_then(|i| i.child.take());
        if let Some(ch) = child.as_mut() {
            ch.kill(signal, StopProtocol::GroupTerm)
                .await
                .map_err(TunnelError::Io)?;
            graceful_reap(ch, self.policy.stop_grace).await;
        }
        let window = self.policy.health_check_window.as_secs();
        if let Some(i) = self.instances.get_mut(site) {
            let base = format!("no Cloudflare edge connection within {window}s");
            i.last_error = Some(with_log_error(base, &i.logfile));
            i.child = None;
            i.edge_up = false;
            i.timed_out = true;
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
    /// SIGKILL, drop the instance, and remove its pidfile. No-op if there is
    /// none. A child that already exited, or vanished before the signal
    /// landed (`ESRCH`), counts as stopped. Bounded by the stop grace.
    ///
    /// # Errors
    ///
    /// Signalling the child failed.
    pub async fn stop(&mut self, site: &str) -> Result<(), TunnelError> {
        let Some(mut inst) = self.instances.remove(site) else {
            return Ok(());
        };
        let _ = std::fs::remove_file(&inst.pidfile);
        let Some(mut child) = inst.child.take() else {
            return Ok(());
        };
        if matches!(child.try_wait(), Ok(Some(_))) {
            return Ok(());
        }
        match child.kill(KillSignal::Term, StopProtocol::GroupTerm).await {
            Ok(()) => {}
            Err(e) if yerd_supervise::is_no_such_process(&e) => return Ok(()),
            Err(e) => return Err(TunnelError::Io(e)),
        }
        graceful_reap(&mut child, self.policy.stop_grace).await;
        Ok(())
    }

    /// Stop `site`'s tunnel only when its current generation is `generation`;
    /// the check and the stop happen under the same borrow (the caller's one
    /// manager lock). Returns whether it stopped anything, so a caller cleaning
    /// up its own failed start can never stop someone else's replacement.
    ///
    /// # Errors
    ///
    /// Signalling the child failed.
    pub async fn stop_if_generation(
        &mut self,
        site: &str,
        generation: u64,
    ) -> Result<bool, TunnelError> {
        if !self
            .instances
            .get(site)
            .is_some_and(|i| i.generation == generation)
        {
            return Ok(false);
        }
        self.stop(site).await?;
        Ok(true)
    }

    /// Refuse every later [`Self::begin`], then stop every supervised tunnel in
    /// deterministic order.
    ///
    /// # Errors
    ///
    /// The first signalling failure; every instance is still attempted.
    pub async fn shutdown(&mut self) -> Result<(), TunnelError> {
        self.closed = true;
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
            let alive = inst
                .child
                .as_mut()
                .is_some_and(|ch| matches!(ch.try_wait(), Ok(None)));
            let pid = if alive {
                inst.child.as_ref().map(ChildHandle::id)
            } else {
                None
            };
            out.push(TunnelSnapshot {
                site: site.clone(),
                kind: inst.kind,
                state: report_state(inst.state, inst.ever_ready, inst.edge_up),
                pid,
                url: inst.url.clone(),
                generation: inst.generation,
                last_error: inst.last_error.clone(),
                failures: match inst.state {
                    PoolState::Failed { attempts, .. } => attempts,
                    _ => 0,
                },
            });
        }
        out
    }
}

/// Whether `spec` would run the same command as `inst` (binary, args, env; the
/// metrics source and file paths are not compared).
fn same_command<Ch: ChildHandle>(inst: &Instance<Ch>, spec: &LaunchSpec) -> bool {
    inst.binary == spec.binary && inst.args == spec.args && inst.env == spec.env
}

/// Map the FSM state plus readiness history to the reported [`TunnelState`]:
/// `Starting` is `Connecting` until the instance has been online once, and
/// `Reconnecting` after; `Running` with the edge down is `Reconnecting` too.
fn report_state(state: PoolState, ever_ready: bool, edge_up: bool) -> TunnelState {
    match state {
        PoolState::Running { .. } if edge_up => TunnelState::Running,
        PoolState::Running { .. } => TunnelState::Reconnecting,
        PoolState::Starting { .. } | PoolState::Stopped if ever_ready => TunnelState::Reconnecting,
        PoolState::Starting { .. } | PoolState::Stopped => TunnelState::Connecting,
        PoolState::Failed { .. } | PoolState::Stopping { .. } => TunnelState::Failed,
    }
}

/// How the child exited, if it has: a missing child counts as an unknown exit.
/// An indeterminate `try_wait` counts as still alive.
fn child_exit<Ch: ChildHandle>(child: Option<&mut Ch>) -> Option<ExitReason> {
    match child {
        Some(ch) => ch.try_wait().ok().flatten(),
        None => Some(ExitReason::Unknown),
    }
}

/// Record an observed exit: drop the child, note the reason plus the log's last
/// error line, and feed `Crashed` to the FSM.
fn record_crash<Ch: ChildHandle>(inst: &mut Instance<Ch>, reason: ExitReason) {
    inst.child = None;
    inst.edge_up = false;
    inst.last_error = Some(with_log_error(
        format!("cloudflared exited ({reason})"),
        &inst.logfile,
    ));
    inst.pending = Event::Crashed { reason };
}

/// `base`, followed by the last `ERR` line of `logfile` when there is one.
fn with_log_error(base: String, logfile: &Path) -> String {
    match last_error_line(&read_file_lossy(logfile)) {
        Some(line) => format!("{base}: {line}"),
        None => base,
    }
}

/// Best-effort pidfile write; the reaper treats a missing pidfile as "nothing
/// to reap", so a failure only weakens orphan cleanup.
fn write_pidfile(path: &Path, pid: u32) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, format!("{pid}\n"));
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

/// Build the `cloudflared` command. The inherited environment is cleared first
/// (`env_clear`) and only the pinned `env` is applied, so the daemon's own
/// environment, including any unrelated secrets, never reaches the child; the
/// caller is responsible for forwarding the few variables `cloudflared`
/// legitimately needs. stdout+stderr are redirected to a freshly truncated
/// logfile (so a stale URL from a prior run is not re-parsed) and, on Unix, the
/// child is placed in its own process group so the supervisor's `killpg` reaps
/// it cleanly. The logfile is forced to `0600` on Unix, both at create time and
/// with an explicit `set_permissions` after open so a reused path (`open`'s
/// create-mode is ignored for an existing file) is re-tightened: a Quick
/// tunnel's only access control is its unguessable URL, which is captured here,
/// and a Named tunnel's log carries connection metadata.
fn build_cmd(
    binary: &Path,
    args: &[OsString],
    env: &[(OsString, OsString)],
    logfile: &Path,
) -> Result<StdCommand, TunnelError> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let f = opts.open(logfile).map_err(TunnelError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(TunnelError::Io)?;
    }
    let f2 = f.try_clone().map_err(TunnelError::Io)?;
    let mut cmd = StdCommand::new(binary);
    cmd.args(args);
    cmd.env_clear();
    for (k, v) in env {
        cmd.env(k, v);
    }
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
    use std::net::Ipv4Addr;
    use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
    use std::sync::Mutex;

    const URL_LINE: &str = "INF |  https://calm-river-1234.trycloudflare.com  |\n";

    struct FakeClock;
    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            Instant::now()
        }
    }

    type ExitSlot = Arc<Mutex<Option<ExitReason>>>;
    type EnvLog = Arc<Mutex<Vec<Vec<(OsString, OsString)>>>>;

    /// A child that stays alive until killed or until its exit slot is set.
    struct FakeChild {
        pid: u32,
        exit: ExitSlot,
    }

    #[async_trait]
    impl ChildHandle for FakeChild {
        fn id(&self) -> u32 {
            self.pid
        }
        fn try_wait(&mut self) -> Result<Option<ExitReason>, io::Error> {
            Ok(*self.exit.lock().unwrap())
        }
        async fn wait(&mut self) -> Result<ExitReason, io::Error> {
            Ok(ExitReason::Code(0))
        }
        async fn kill(
            &mut self,
            _signal: KillSignal,
            _protocol: StopProtocol,
        ) -> Result<(), io::Error> {
            *self.exit.lock().unwrap() = Some(ExitReason::Code(0));
            Ok(())
        }
    }

    /// Writes canned log contents on each spawn (standing in for cloudflared's
    /// own output) and records every spawn's env and exit slot. With `fail`
    /// set, spawning errors instead.
    #[derive(Clone, Default)]
    struct FakeSpawner {
        logfile: PathBuf,
        contents: String,
        fail: Arc<AtomicBool>,
        spawns: Arc<AtomicU32>,
        envs: EnvLog,
        exits: Arc<Mutex<Vec<ExitSlot>>>,
    }

    impl FakeSpawner {
        fn new(logfile: PathBuf, contents: &str) -> Self {
            Self {
                logfile,
                contents: contents.to_owned(),
                ..Self::default()
            }
        }

        fn spawn_count(&self) -> u32 {
            self.spawns.load(Ordering::SeqCst)
        }

        fn crash_latest(&self, reason: ExitReason) {
            let exits = self.exits.lock().unwrap();
            *exits.last().unwrap().lock().unwrap() = Some(reason);
        }

        fn metrics_env(&self, spawn: usize) -> Option<OsString> {
            self.envs.lock().unwrap()[spawn]
                .iter()
                .find(|(k, _)| k == "TUNNEL_METRICS")
                .map(|(_, v)| v.clone())
        }
    }

    impl ProcessSpawner for FakeSpawner {
        type Child = FakeChild;
        fn spawn(&self, cmd: StdCommand) -> Result<FakeChild, io::Error> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no such binary"));
            }
            std::fs::write(&self.logfile, self.contents.as_bytes())?;
            let n = self.spawns.fetch_add(1, Ordering::SeqCst) + 1;
            let env = cmd
                .get_envs()
                .filter_map(|(k, v)| v.map(|v| (k.to_os_string(), v.to_os_string())))
                .collect();
            self.envs.lock().unwrap().push(env);
            let exit: ExitSlot = Arc::new(Mutex::new(None));
            self.exits.lock().unwrap().push(Arc::clone(&exit));
            Ok(FakeChild {
                pid: 4000 + n,
                exit,
            })
        }
    }

    /// A child that has already exited (every `try_wait` reports a crash).
    struct DeadChild;

    #[async_trait]
    impl ChildHandle for DeadChild {
        fn id(&self) -> u32 {
            7
        }
        fn try_wait(&mut self) -> Result<Option<ExitReason>, io::Error> {
            Ok(Some(ExitReason::Code(1)))
        }
        async fn wait(&mut self) -> Result<ExitReason, io::Error> {
            Ok(ExitReason::Code(1))
        }
        async fn kill(
            &mut self,
            _signal: KillSignal,
            _protocol: StopProtocol,
        ) -> Result<(), io::Error> {
            Ok(())
        }
    }

    /// Spawns children that are immediately dead, counting spawns.
    #[derive(Clone, Default)]
    struct CrashSpawner {
        spawns: Arc<AtomicU32>,
    }

    impl ProcessSpawner for CrashSpawner {
        type Child = DeadChild;
        fn spawn(&self, _cmd: StdCommand) -> Result<DeadChild, io::Error> {
            self.spawns.fetch_add(1, Ordering::SeqCst);
            Ok(DeadChild)
        }
    }

    /// A child whose liveness and kill outcome are fixed up front; every kill
    /// attempt is counted.
    struct ScriptedChild {
        exited: bool,
        kill_error: Option<io::ErrorKind>,
        kills: Arc<AtomicU32>,
    }

    #[async_trait]
    impl ChildHandle for ScriptedChild {
        fn id(&self) -> u32 {
            9
        }
        fn try_wait(&mut self) -> Result<Option<ExitReason>, io::Error> {
            Ok(self.exited.then_some(ExitReason::Code(0)))
        }
        async fn wait(&mut self) -> Result<ExitReason, io::Error> {
            Ok(ExitReason::Code(0))
        }
        async fn kill(
            &mut self,
            _signal: KillSignal,
            _protocol: StopProtocol,
        ) -> Result<(), io::Error> {
            self.kills.fetch_add(1, Ordering::SeqCst);
            self.kill_error
                .map_or(Ok(()), |kind| Err(io::Error::from(kind)))
        }
    }

    #[derive(Clone, Default)]
    struct ScriptedSpawner {
        exited: bool,
        kill_error: Option<io::ErrorKind>,
        kills: Arc<AtomicU32>,
    }

    impl ProcessSpawner for ScriptedSpawner {
        type Child = ScriptedChild;
        fn spawn(&self, _cmd: StdCommand) -> Result<ScriptedChild, io::Error> {
            Ok(ScriptedChild {
                exited: self.exited,
                kill_error: self.kill_error,
                kills: Arc::clone(&self.kills),
            })
        }
    }

    async fn stop_scripted(
        exited: bool,
        kill_error: Option<io::ErrorKind>,
    ) -> (Result<(), TunnelError>, u32) {
        let tmp = tempfile::tempdir().unwrap();
        let spawner = ScriptedSpawner {
            exited,
            kill_error,
            ..ScriptedSpawner::default()
        };
        let mut mgr = TunnelManager::new(spawner.clone(), FakeClock);
        mgr.begin("app", spec(tmp.path(), "app"), TunnelKind::Named)
            .await
            .unwrap()
            .unwrap();
        let result = mgr.stop("app").await;
        assert!(mgr.snapshots().is_empty());
        (result, spawner.kills.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn stop_skips_the_kill_for_an_already_exited_child() {
        let (result, kills) = stop_scripted(true, Some(io::ErrorKind::Other)).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(kills, 0);
    }

    #[tokio::test]
    async fn stop_treats_a_vanished_process_as_stopped() {
        let (result, kills) = stop_scripted(false, Some(io::ErrorKind::NotFound)).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(kills, 1);
    }

    #[tokio::test]
    async fn stop_still_reports_other_kill_failures() {
        let (result, _) = stop_scripted(false, Some(io::ErrorKind::PermissionDenied)).await;
        assert!(matches!(result, Err(TunnelError::Io(_))), "{result:?}");
    }

    struct Calls(Arc<AtomicU16>);

    impl Calls {
        fn count(&self) -> u16 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn counting_metrics() -> (MetricsAddrSource, Calls) {
        let calls = Arc::new(AtomicU16::new(0));
        let counter = Arc::clone(&calls);
        let source: MetricsAddrSource = Arc::new(move || {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(SocketAddr::from((Ipv4Addr::LOCALHOST, 9000 + n)))
        });
        (source, Calls(calls))
    }

    fn spec_in(dir: &Path, key: &str, metrics_addr: MetricsAddrSource) -> LaunchSpec {
        LaunchSpec {
            binary: PathBuf::from("/usr/bin/cloudflared"),
            args: vec![],
            env: vec![],
            logfile: dir.join(format!("{key}.log")),
            metrics_addr,
            pidfile: dir.join(format!("{key}.pid")),
        }
    }

    fn spec(dir: &Path, key: &str) -> LaunchSpec {
        spec_in(dir, key, counting_metrics().0)
    }

    /// Drive `key` until a step other than `Continue`/`Sleep`/`Probe` appears,
    /// answering every probe with `ok`. Bounded so a regression fails loudly.
    async fn drive<S: ProcessSpawner, C: Clock>(
        mgr: &mut TunnelManager<S, C>,
        key: &str,
        generation: u64,
        ok: bool,
    ) -> Step {
        for _ in 0..200 {
            let mut step = mgr.advance(key, generation).await.unwrap();
            if let Step::Probe(_) = step {
                step = mgr.record_probe(key, generation, ok);
            }
            match step {
                Step::Continue | Step::Sleep(_) | Step::Probe(_) => {}
                done => return done,
            }
        }
        panic!("tunnel {key} never settled");
    }

    async fn ready_quick(
        dir: &Path,
        contents: &str,
    ) -> (TunnelManager<FakeSpawner, FakeClock>, FakeSpawner, u64) {
        let spawner = FakeSpawner::new(dir.join("app.log"), contents);
        let mut mgr = TunnelManager::new(spawner.clone(), FakeClock);
        let generation = mgr
            .begin("app", spec(dir, "app"), TunnelKind::Quick)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(drive(&mut mgr, "app", generation, true).await, Step::Ready);
        (mgr, spawner, generation)
    }

    fn only<S: ProcessSpawner, C: Clock>(mgr: &mut TunnelManager<S, C>) -> TunnelSnapshot {
        let mut snaps = mgr.snapshots();
        assert_eq!(snaps.len(), 1);
        snaps.remove(0)
    }

    #[test]
    fn report_state_table() {
        let running = PoolState::Running { pid: 1 };
        let starting = PoolState::Starting {
            attempts: 1,
            pid: Some(1),
        };
        let failed = PoolState::Failed {
            last_exit: ExitReason::Code(1),
            attempts: 1,
        };
        for (state, ever_ready, edge_up, expected) in [
            (running, true, true, TunnelState::Running),
            (running, true, false, TunnelState::Reconnecting),
            (starting, false, false, TunnelState::Connecting),
            (starting, true, false, TunnelState::Reconnecting),
            (PoolState::Stopped, false, false, TunnelState::Connecting),
            (failed, false, false, TunnelState::Failed),
            (failed, true, false, TunnelState::Failed),
            (
                PoolState::Stopping { sigkilled: false },
                true,
                true,
                TunnelState::Failed,
            ),
        ] {
            assert_eq!(
                report_state(state, ever_ready, edge_up),
                expected,
                "{state:?} ever_ready={ever_ready} edge_up={edge_up}"
            );
        }
    }

    #[tokio::test]
    async fn begin_then_advance_captures_url_and_reports_running() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut mgr, _spawner, generation) = ready_quick(tmp.path(), URL_LINE).await;

        let snap = only(&mut mgr);
        assert_eq!(snap.site, "app");
        assert_eq!(snap.kind, TunnelKind::Quick);
        assert_eq!(snap.state, TunnelState::Running);
        assert_eq!(snap.generation, generation);
        assert_eq!(snap.last_error, None);
        assert_eq!(
            snap.url.as_deref(),
            Some("https://calm-river-1234.trycloudflare.com")
        );
    }

    #[tokio::test]
    async fn a_repeat_begin_never_restarts_a_running_tunnel() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut mgr, spawner, generation) = ready_quick(tmp.path(), URL_LINE).await;
        let before = only(&mut mgr);

        let again = mgr
            .begin("app", spec(tmp.path(), "app"), TunnelKind::Quick)
            .await
            .unwrap();

        assert_eq!(again, None);
        let after = only(&mut mgr);
        assert_eq!(after.generation, generation);
        assert_eq!(after.pid, before.pid);
        assert_eq!(spawner.spawn_count(), 1);
    }

    #[tokio::test]
    async fn quick_readiness_needs_both_the_url_and_a_probe_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let spawner = FakeSpawner::new(tmp.path().join("app.log"), "INF connecting\n");
        let mut mgr = TunnelManager::new(spawner, FakeClock);
        let generation = mgr
            .begin("app", spec(tmp.path(), "app"), TunnelKind::Quick)
            .await
            .unwrap()
            .unwrap();

        for _ in 0..5 {
            let step = mgr.advance("app", generation).await.unwrap();
            assert!(matches!(step, Step::Continue | Step::Sleep(_)), "{step:?}");
        }
        assert_eq!(only(&mut mgr).state, TunnelState::Connecting);
        assert_eq!(
            mgr.record_probe("app", generation, true),
            Step::Sleep(READINESS_GAP)
        );

        std::fs::write(tmp.path().join("app.log"), URL_LINE).unwrap();
        let probe = loop {
            match mgr.advance("app", generation).await.unwrap() {
                Step::Probe(addr) => break addr,
                Step::Continue | Step::Sleep(_) => {}
                other => panic!("unexpected {other:?}"),
            }
        };
        assert_eq!(probe, SocketAddr::from((Ipv4Addr::LOCALHOST, 9001)));
        assert_eq!(
            mgr.record_probe("app", generation, false),
            Step::Sleep(READINESS_GAP)
        );
        assert_eq!(only(&mut mgr).state, TunnelState::Connecting);
        assert_eq!(drive(&mut mgr, "app", generation, true).await, Step::Ready);
        assert_eq!(only(&mut mgr).state, TunnelState::Running);
    }

    #[tokio::test]
    async fn named_tunnel_is_ready_on_a_probe_ok_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let spawner = FakeSpawner::new(tmp.path().join("shop.log"), "INF starting\n");
        let mut mgr = TunnelManager::new(spawner, FakeClock);
        let generation = mgr
            .begin("shop", spec(tmp.path(), "shop"), TunnelKind::Named)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(drive(&mut mgr, "shop", generation, true).await, Step::Ready);
        let snap = only(&mut mgr);
        assert_eq!(snap.kind, TunnelKind::Named);
        assert_eq!(snap.state, TunnelState::Running);
    }

    #[tokio::test]
    async fn ready_is_returned_once_then_the_monitor_takes_over() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut mgr, spawner, generation) = ready_quick(tmp.path(), URL_LINE).await;

        for _ in 0..10 {
            let step = mgr.advance("app", generation).await.unwrap();
            assert!(matches!(step, Step::Probe(_)), "{step:?}");
            assert_eq!(
                mgr.record_probe("app", generation, true),
                Step::Sleep(MONITOR_INTERVAL)
            );
        }
        assert_eq!(spawner.spawn_count(), 1);
    }

    #[tokio::test]
    async fn an_edge_outage_while_alive_reports_reconnecting_without_respawn() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut mgr, spawner, generation) = ready_quick(tmp.path(), URL_LINE).await;

        assert!(matches!(
            mgr.advance("app", generation).await.unwrap(),
            Step::Probe(_)
        ));
        mgr.record_probe("app", generation, false);
        assert_eq!(only(&mut mgr).state, TunnelState::Reconnecting);

        assert!(matches!(
            mgr.advance("app", generation).await.unwrap(),
            Step::Probe(_)
        ));
        mgr.record_probe("app", generation, true);
        assert_eq!(only(&mut mgr).state, TunnelState::Running);
        assert_eq!(spawner.spawn_count(), 1);
    }

    #[tokio::test]
    async fn a_running_child_that_exits_is_restarted_with_its_error_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let contents = format!("{URL_LINE}2026-01-01T00:00:00Z ERR lost the edge\n");
        let (mut mgr, spawner, generation) = ready_quick(tmp.path(), &contents).await;

        spawner.crash_latest(ExitReason::Code(1));
        assert_eq!(
            mgr.advance("app", generation).await.unwrap(),
            Step::Continue
        );
        assert_eq!(
            mgr.advance("app", generation).await.unwrap(),
            Step::Sleep(Duration::from_secs(1))
        );
        let failed = only(&mut mgr);
        assert_eq!(failed.state, TunnelState::Failed);
        assert_eq!(
            failed.last_error.as_deref(),
            Some("cloudflared exited (exit 1): lost the edge")
        );

        assert_eq!(
            mgr.advance("app", generation).await.unwrap(),
            Step::Continue
        );
        assert_eq!(spawner.spawn_count(), 2);
        let respawned = only(&mut mgr);
        assert_eq!(respawned.state, TunnelState::Reconnecting);
        assert_eq!(respawned.url, None);
        assert_eq!(respawned.generation, generation);

        assert_eq!(drive(&mut mgr, "app", generation, true).await, Step::Ready);
        let back = only(&mut mgr);
        assert_eq!(back.state, TunnelState::Running);
        assert_eq!(back.last_error, None);
        assert!(back.url.is_some());
    }

    #[tokio::test]
    async fn a_crashing_tunnel_keeps_retrying_past_the_old_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let spawner = CrashSpawner::default();
        let mut mgr = TunnelManager::new(spawner.clone(), FakeClock);
        let generation = mgr
            .begin("dead", spec(tmp.path(), "dead"), TunnelKind::Quick)
            .await
            .unwrap()
            .unwrap();

        for _ in 0..100 {
            let step = mgr.advance("dead", generation).await.unwrap();
            assert!(matches!(step, Step::Continue | Step::Sleep(_)), "{step:?}");
        }
        assert!(spawner.spawns.load(Ordering::SeqCst) > 10);
        let snap = only(&mut mgr);
        assert!(matches!(
            snap.state,
            TunnelState::Failed | TunnelState::Connecting
        ));
        assert!(snap.last_error.is_some());
    }

    #[tokio::test]
    async fn a_stale_generation_is_fenced_off() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut mgr, _spawner, first) = ready_quick(tmp.path(), URL_LINE).await;

        mgr.stop("app").await.unwrap();
        let second = mgr
            .begin("app", spec(tmp.path(), "app"), TunnelKind::Quick)
            .await
            .unwrap()
            .unwrap();

        assert!(second > first);
        assert_eq!(mgr.advance("app", first).await.unwrap(), Step::Gone);
        assert_eq!(mgr.record_probe("app", first, true), Step::Gone);
        assert_eq!(only(&mut mgr).state, TunnelState::Connecting);
        assert_eq!(drive(&mut mgr, "app", second, true).await, Step::Ready);
    }

    #[tokio::test]
    async fn metrics_address_is_picked_per_spawn_and_passed_in_the_env() {
        let tmp = tempfile::tempdir().unwrap();
        let spawner = FakeSpawner::new(tmp.path().join("app.log"), URL_LINE);
        let mut mgr = TunnelManager::new(spawner.clone(), FakeClock);
        let (source, calls) = counting_metrics();
        let generation = mgr
            .begin("app", spec_in(tmp.path(), "app", source), TunnelKind::Quick)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(drive(&mut mgr, "app", generation, true).await, Step::Ready);
        assert_eq!(calls.count(), 1);

        spawner.crash_latest(ExitReason::Code(1));
        assert_eq!(drive(&mut mgr, "app", generation, true).await, Step::Ready);

        assert_eq!(calls.count(), 2);
        assert_eq!(spawner.metrics_env(0), Some("127.0.0.1:9001".into()));
        assert_eq!(spawner.metrics_env(1), Some("127.0.0.1:9002".into()));
    }

    #[tokio::test]
    async fn begin_after_shutdown_is_refused_and_spawns_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let spawner = FakeSpawner::new(tmp.path().join("app.log"), URL_LINE);
        let mut mgr = TunnelManager::new(spawner.clone(), FakeClock);
        mgr.shutdown().await.unwrap();

        let err = mgr
            .begin("app", spec(tmp.path(), "app"), TunnelKind::Quick)
            .await
            .unwrap_err();

        assert!(matches!(err, TunnelError::ShuttingDown), "{err:?}");
        assert_eq!(spawner.spawn_count(), 0);
        assert!(mgr.snapshots().is_empty());
    }

    #[tokio::test]
    async fn begin_during_backoff_keeps_the_retrying_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut mgr, spawner, generation) = ready_quick(tmp.path(), URL_LINE).await;
        spawner.crash_latest(ExitReason::Code(1));
        mgr.advance("app", generation).await.unwrap();
        mgr.advance("app", generation).await.unwrap();
        let failed = only(&mut mgr);
        assert_eq!(failed.state, TunnelState::Failed);

        let again = mgr
            .begin("app", spec(tmp.path(), "app"), TunnelKind::Quick)
            .await
            .unwrap();

        assert_eq!(again, None);
        let kept = only(&mut mgr);
        assert_eq!(kept.generation, generation);
        assert_eq!(kept.last_error, failed.last_error);
        assert!(kept.last_error.is_some());
        assert_eq!(
            mgr.advance("app", generation).await.unwrap(),
            Step::Continue
        );
        assert_eq!(only(&mut mgr).state, TunnelState::Reconnecting);
    }

    async fn failed_quick(dir: &Path) -> (TunnelManager<FakeSpawner, FakeClock>, u64) {
        let (mut mgr, spawner, generation) = ready_quick(dir, URL_LINE).await;
        spawner.crash_latest(ExitReason::Code(1));
        mgr.advance("app", generation).await.unwrap();
        mgr.advance("app", generation).await.unwrap();
        let failed = only(&mut mgr);
        assert_eq!(failed.state, TunnelState::Failed);
        assert_eq!(failed.failures, 1);
        (mgr, generation)
    }

    type SpecChange = fn(&mut LaunchSpec);

    #[tokio::test]
    async fn begin_replaces_a_failed_instance_whose_command_changed() {
        let cases: &[(SpecChange, bool)] = &[
            (|s| s.binary = PathBuf::from("/opt/yerd/cloudflared"), true),
            (|s| s.args = vec!["tunnel".into()], true),
            (|s| s.env = vec![("HOME".into(), "/elsewhere".into())], true),
            (|s| s.metrics_addr = counting_metrics().0, false),
        ];
        for (change, replaced) in cases {
            let tmp = tempfile::tempdir().unwrap();
            let (mut mgr, generation) = failed_quick(tmp.path()).await;
            let mut next = spec(tmp.path(), "app");
            change(&mut next);

            let again = mgr.begin("app", next, TunnelKind::Quick).await.unwrap();

            assert_eq!(again.is_some(), *replaced);
            let snap = only(&mut mgr);
            if *replaced {
                assert!(snap.generation > generation);
                assert_eq!(mgr.advance("app", generation).await.unwrap(), Step::Gone);
            } else {
                assert_eq!(snap.generation, generation);
            }
        }
    }

    #[tokio::test]
    async fn snapshot_failures_count_consecutive_failed_attempts() {
        let tmp = tempfile::tempdir().unwrap();
        let mut mgr = TunnelManager::new(CrashSpawner::default(), FakeClock);
        let generation = mgr
            .begin("dead", spec(tmp.path(), "dead"), TunnelKind::Quick)
            .await
            .unwrap()
            .unwrap();
        let mut seen = Vec::new();
        for _ in 0..40 {
            mgr.advance("dead", generation).await.unwrap();
            let snap = only(&mut mgr);
            if snap.state == TunnelState::Failed && seen.last() != Some(&snap.failures) {
                seen.push(snap.failures);
            }
        }
        assert_eq!(seen.get(..3), Some(&[1, 2, 3][..]));
    }

    #[tokio::test]
    async fn stop_if_generation_only_stops_the_matching_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut mgr, _spawner, generation) = ready_quick(tmp.path(), URL_LINE).await;

        assert!(!mgr.stop_if_generation("app", generation + 1).await.unwrap());
        assert_eq!(only(&mut mgr).state, TunnelState::Running);

        assert!(mgr.stop_if_generation("app", generation).await.unwrap());
        assert!(mgr.snapshots().is_empty());
        assert!(!mgr.stop_if_generation("app", generation).await.unwrap());
    }

    #[tokio::test]
    async fn pidfile_is_written_on_spawn_and_removed_on_stop() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut mgr, _spawner, _generation) = ready_quick(tmp.path(), URL_LINE).await;
        let pidfile = tmp.path().join("app.pid");
        let pid = only(&mut mgr).pid.unwrap();

        assert_eq!(
            std::fs::read_to_string(&pidfile).unwrap(),
            format!("{pid}\n")
        );

        mgr.stop("app").await.unwrap();
        assert!(!pidfile.exists());
        mgr.stop("nope").await.unwrap();
    }

    #[tokio::test]
    async fn a_spawn_error_is_recorded_and_retried_not_returned() {
        let tmp = tempfile::tempdir().unwrap();
        let spawner = FakeSpawner::new(tmp.path().join("app.log"), URL_LINE);
        spawner.fail.store(true, Ordering::SeqCst);
        let mut mgr = TunnelManager::new(spawner.clone(), FakeClock);

        let generation = mgr
            .begin("app", spec(tmp.path(), "app"), TunnelKind::Quick)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            mgr.advance("app", generation).await.unwrap(),
            Step::Sleep(_)
        ));

        let snap = only(&mut mgr);
        assert_eq!(snap.state, TunnelState::Failed);
        assert!(
            snap.last_error
                .as_deref()
                .is_some_and(|e| e.contains("no such binary")),
            "{:?}",
            snap.last_error
        );

        spawner.fail.store(false, Ordering::SeqCst);
        assert_eq!(drive(&mut mgr, "app", generation, true).await, Step::Ready);
        assert_eq!(spawner.spawn_count(), 1);
    }

    #[tokio::test]
    async fn a_metrics_address_error_is_recorded_and_retried() {
        let tmp = tempfile::tempdir().unwrap();
        let spawner = FakeSpawner::new(tmp.path().join("app.log"), URL_LINE);
        let mut mgr = TunnelManager::new(spawner.clone(), FakeClock);
        let failing: MetricsAddrSource = Arc::new(|| Err(io::Error::other("no ports")));

        let generation = mgr
            .begin(
                "app",
                spec_in(tmp.path(), "app", failing),
                TunnelKind::Quick,
            )
            .await
            .unwrap()
            .unwrap();
        mgr.advance("app", generation).await.unwrap();

        let snap = only(&mut mgr);
        assert_eq!(snap.state, TunnelState::Failed);
        assert_eq!(
            snap.last_error.as_deref(),
            Some("could not pick a metrics address: no ports")
        );
        assert_eq!(spawner.spawn_count(), 0);
    }

    #[test]
    fn launch_spec_debug_omits_env_values() {
        let mut spec = spec(Path::new("/t"), "app");
        spec.env = vec![("TUNNEL_ORIGIN_CERT".into(), "secret-value".into())];
        let shown = format!("{spec:?}");
        assert!(shown.contains("/t/app.pid"));
        assert!(!shown.contains("secret-value"));
    }
}
