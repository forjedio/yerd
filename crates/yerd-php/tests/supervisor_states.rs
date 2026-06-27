//! End-to-end driver tests with fakes for `ProcessSpawner`, `Clock`,
//! `HealthProbe`, and `ChildHandle`. Verifies that `PhpManager::ensure`
//! drives the supervisor through the happy path, crash + recovery,
//! permanent failure, and clean stop.
//!
//! Live FPM coverage lands in `bin/yerdd`'s integration suite; this
//! test stays fakes-only so it passes on every CI target.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::Mutex;

use yerd_core::PhpVersion;
use yerd_php::pure::supervisor::{KillSignal, StopProtocol, SupervisorPolicy};
use yerd_php::{
    ChildHandle, Clock, ExitReason, HealthProbe, Listen, PhpError, PhpManager, PoolRunState,
    ProcessSpawner,
};
use yerd_platform::{ActivePortBinder, PlatformDirs};

// ─── Fakes ──────────────────────────────────────────────────────────

/// Programmable child outcome.
#[derive(Clone)]
enum ChildBehavior {
    /// `wait()` resolves immediately with this exit reason.
    Crashes(ExitReason),
    /// `wait()` blocks forever (until killed).
    Lives,
    /// `wait()` blocks forever, but `kill()` flips it to "exited".
    LivesUntilKilled,
    /// `wait()` blocks forever (so `ensure` stores the pool as `Running`), but
    /// `try_wait()` immediately reports an exit, modelling a master that died
    /// *after* it was stored healthy, which `snapshots` must report as `Failed`.
    LivesButTryWaitReportsExited(ExitReason),
}

struct FakeChild {
    pid: u32,
    behavior: Arc<Mutex<ChildBehavior>>,
    kills: Arc<Mutex<Vec<KillSignal>>>,
    killed_notify: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl ChildHandle for FakeChild {
    fn id(&self) -> u32 {
        self.pid
    }

    fn try_wait(&mut self) -> Result<Option<ExitReason>, io::Error> {
        let guard = self.behavior.try_lock().ok();
        match guard.as_deref() {
            Some(ChildBehavior::Crashes(r) | ChildBehavior::LivesButTryWaitReportsExited(r)) => {
                Ok(Some(*r))
            }
            _ => Ok(None),
        }
    }

    async fn wait(&mut self) -> Result<ExitReason, io::Error> {
        loop {
            let behavior = self.behavior.lock().await.clone();
            match behavior {
                ChildBehavior::Crashes(r) => return Ok(r),
                ChildBehavior::Lives | ChildBehavior::LivesButTryWaitReportsExited(_) => {
                    std::future::pending::<()>().await;
                }
                ChildBehavior::LivesUntilKilled => {
                    self.killed_notify.notified().await;
                }
            }
        }
    }

    async fn kill(&mut self, signal: KillSignal, _protocol: StopProtocol) -> Result<(), io::Error> {
        self.kills.lock().await.push(signal);
        let mut b = self.behavior.lock().await;
        if matches!(*b, ChildBehavior::LivesUntilKilled) {
            *b = ChildBehavior::Crashes(ExitReason::Signal(15));
            self.killed_notify.notify_waiters();
        }
        Ok(())
    }
}

/// Plan for the n-th spawn (1-indexed).
#[derive(Clone)]
struct SpawnPlan {
    pid: u32,
    behavior: ChildBehavior,
}

struct FakeSpawner {
    plans: Mutex<std::collections::VecDeque<SpawnPlan>>,
    spawn_count: Mutex<usize>,
    last_kills: Arc<Mutex<Vec<KillSignal>>>,
}

impl FakeSpawner {
    fn new(plans: Vec<SpawnPlan>) -> Self {
        Self {
            plans: Mutex::new(plans.into()),
            spawn_count: Mutex::new(0),
            last_kills: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn spawn_count(&self) -> usize {
        *self.spawn_count.lock().await
    }

    fn kills_handle(&self) -> Arc<Mutex<Vec<KillSignal>>> {
        Arc::clone(&self.last_kills)
    }
}

impl ProcessSpawner for FakeSpawner {
    type Child = FakeChild;

    fn spawn(&self, _cmd: std::process::Command) -> Result<FakeChild, io::Error> {
        let mut plans = self
            .plans
            .try_lock()
            .map_err(|_| io::Error::other("spawn: plans lock contended"))?;
        let mut counter = self
            .spawn_count
            .try_lock()
            .map_err(|_| io::Error::other("spawn: count lock contended"))?;
        let plan = plans
            .pop_front()
            .ok_or_else(|| io::Error::other("spawn: no more plans"))?;
        *counter += 1;
        Ok(FakeChild {
            pid: plan.pid,
            behavior: Arc::new(Mutex::new(plan.behavior)),
            kills: Arc::clone(&self.last_kills),
            killed_notify: Arc::new(tokio::sync::Notify::new()),
        })
    }
}

struct FakeClock;
impl Clock for FakeClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Programmable probe. Each call pulls the next outcome from the queue;
/// when the queue empties, returns the `tail` outcome forever.
struct FakeProbe {
    sequence: Mutex<std::collections::VecDeque<Result<(), io::ErrorKind>>>,
    tail: Result<(), io::ErrorKind>,
}

impl FakeProbe {
    fn always_ok() -> Self {
        Self {
            sequence: Mutex::new(std::collections::VecDeque::new()),
            tail: Ok(()),
        }
    }

    fn always_refused() -> Self {
        Self {
            sequence: Mutex::new(std::collections::VecDeque::new()),
            tail: Err(io::ErrorKind::ConnectionRefused),
        }
    }
}

#[async_trait]
impl HealthProbe for FakeProbe {
    async fn probe(&self, _listen: &Listen) -> Result<(), io::Error> {
        let mut seq = self.sequence.lock().await;
        let outcome = seq.pop_front().unwrap_or(self.tail);
        match outcome {
            Ok(()) => Ok(()),
            Err(kind) => Err(io::Error::from(kind)),
        }
    }
}

fn make_dirs() -> PlatformDirs {
    PlatformDirs {
        config: PathBuf::from("/tmp/yerd-test/cfg"),
        data: PathBuf::from("/tmp/yerd-test/data"),
        state: PathBuf::from("/tmp/yerd-test/state"),
        cache: PathBuf::from("/tmp/yerd-test/cache"),
        runtime: PathBuf::from("/tmp/yerd-test/run"),
    }
}

fn binaries_with(v: PhpVersion) -> BTreeMap<PhpVersion, PathBuf> {
    let mut m = BTreeMap::new();
    m.insert(v, PathBuf::from("/usr/bin/true"));
    m
}

fn make_manager(
    spawner: FakeSpawner,
    probe: FakeProbe,
    v: PhpVersion,
) -> PhpManager<FakeSpawner, FakeClock, FakeProbe> {
    let dirs = make_dirs();
    std::fs::create_dir_all(&dirs.config).unwrap();
    std::fs::create_dir_all(&dirs.state).unwrap();
    std::fs::create_dir_all(&dirs.runtime).unwrap();
    PhpManager::new(
        spawner,
        FakeClock,
        probe,
        dirs,
        ActivePortBinder::new(),
        1234,
        binaries_with(v),
    )
}

// ─── Tests ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_happy_path_returns_listen() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 101,
        behavior: ChildBehavior::Lives,
    }]);
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);

    let listen = mgr.ensure(v).await.unwrap();
    match listen {
        Listen::UnixSocket(p) => assert!(p.to_string_lossy().contains("fpm-8.3-1234.sock")),
        Listen::TcpLoopback(_) => {}
    }

    let _ = mgr.ensure(v).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn set_binaries_makes_a_runtime_install_visible() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 101,
        behavior: ChildBehavior::Lives,
    }]);
    let dirs = make_dirs();
    std::fs::create_dir_all(&dirs.config).unwrap();
    std::fs::create_dir_all(&dirs.state).unwrap();
    std::fs::create_dir_all(&dirs.runtime).unwrap();
    let mut mgr = PhpManager::new(
        spawner,
        FakeClock,
        FakeProbe::always_ok(),
        dirs,
        ActivePortBinder::new(),
        4242,
        BTreeMap::new(),
    );

    assert!(matches!(
        mgr.ensure(v).await,
        Err(PhpError::VersionNotInstalled { .. })
    ));

    let mut binaries = BTreeMap::new();
    binaries.insert(v, PathBuf::from("/usr/bin/true"));
    mgr.set_binaries(binaries);

    assert!(mgr.ensure(v).await.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_creates_missing_state_dir_for_logs() {
    let v = PhpVersion::new(8, 3);
    let tmp = tempfile::tempdir().unwrap();
    let dirs = yerd_platform::PlatformDirs {
        config: tmp.path().join("config"),
        data: tmp.path().join("data"),
        state: tmp.path().join("state"),
        cache: tmp.path().join("cache"),
        runtime: tmp.path().join("run"),
    };
    std::fs::create_dir_all(&dirs.runtime).unwrap();
    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 101,
        behavior: ChildBehavior::Lives,
    }]);
    let mut binaries = BTreeMap::new();
    binaries.insert(v, PathBuf::from("/usr/bin/true"));
    let mut mgr = PhpManager::new(
        spawner,
        FakeClock,
        FakeProbe::always_ok(),
        dirs.clone(),
        ActivePortBinder::new(),
        4242,
        binaries,
    );

    assert!(mgr.ensure(v).await.is_ok());
    assert!(
        dirs.state.is_dir(),
        "ensure() should have created the state dir"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn snapshots_empty_when_nothing_started() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![]);
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);
    assert!(mgr.snapshots().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn snapshots_report_running_pool_with_pid() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 101,
        behavior: ChildBehavior::Lives,
    }]);
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);
    mgr.ensure(v).await.unwrap();

    let snaps = mgr.snapshots();
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].version, v);
    assert_eq!(snaps[0].state, PoolRunState::Running);
    assert_eq!(snaps[0].pid, Some(101));
    assert!(snaps[0].listen.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_recovers_after_one_crash() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![
        SpawnPlan {
            pid: 101,
            behavior: ChildBehavior::Crashes(ExitReason::Code(1)),
        },
        SpawnPlan {
            pid: 102,
            behavior: ChildBehavior::Lives,
        },
    ]);
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);

    let _listen = mgr.ensure(v).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_surfaces_permanent_failure() {
    let v = PhpVersion::new(8, 3);
    let max = SupervisorPolicy::fpm().max_restart_attempts;
    let plans: Vec<SpawnPlan> = (0..=max + 2)
        .map(|i| SpawnPlan {
            pid: 100 + i,
            behavior: ChildBehavior::Crashes(ExitReason::Code(1)),
        })
        .collect();
    let spawner = FakeSpawner::new(plans);
    let mut mgr = make_manager(spawner, FakeProbe::always_refused(), v);

    let err = mgr.ensure(v).await.unwrap_err();
    assert!(
        matches!(err, PhpError::PermanentFailure { .. }),
        "got: {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn stop_kills_running_pool() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 101,
        behavior: ChildBehavior::LivesUntilKilled,
    }]);
    let kills = spawner.kills_handle();
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);

    mgr.ensure(v).await.unwrap();
    mgr.stop(v).await.unwrap();

    let kills_now = kills.lock().await;
    assert!(
        kills_now.contains(&KillSignal::Term),
        "expected at least one SIGTERM, got {kills_now:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn stop_on_unmanaged_version_is_noop() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![]);
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);
    mgr.stop(v).await.unwrap();
    mgr.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_unknown_version_errors() {
    let v = PhpVersion::new(8, 3);
    let other = PhpVersion::new(7, 4);
    let spawner = FakeSpawner::new(vec![]);
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);
    let err = mgr.ensure(other).await.unwrap_err();
    assert!(
        matches!(err, PhpError::VersionNotInstalled { version } if version == other),
        "got: {err:?}"
    );
}

// Silences `dead_code` for `FakeSpawner::spawn_count` so a future test can
// assert spawn counts directly.
#[allow(dead_code)]
async fn _spawn_count_helper(s: &FakeSpawner) -> usize {
    s.spawn_count().await
}

// ─── Added coverage: restart / shutdown / Failed snapshots / idempotency ──

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn restart_stops_then_starts_with_fresh_pid() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![
        SpawnPlan {
            pid: 201,
            behavior: ChildBehavior::LivesUntilKilled,
        },
        SpawnPlan {
            pid: 202,
            behavior: ChildBehavior::Lives,
        },
    ]);
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);

    mgr.ensure(v).await.unwrap();
    assert_eq!(mgr.snapshots()[0].pid, Some(201));

    mgr.restart(v).await.unwrap();
    let snaps = mgr.snapshots();
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].state, PoolRunState::Running);
    assert_eq!(snaps[0].pid, Some(202));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn shutdown_stops_every_running_pool() {
    let v83 = PhpVersion::new(8, 3);
    let v82 = PhpVersion::new(8, 2);
    let spawner = FakeSpawner::new(vec![
        SpawnPlan {
            pid: 301,
            behavior: ChildBehavior::LivesUntilKilled,
        },
        SpawnPlan {
            pid: 302,
            behavior: ChildBehavior::LivesUntilKilled,
        },
    ]);
    let kills = spawner.kills_handle();

    let dirs = make_dirs();
    std::fs::create_dir_all(&dirs.config).unwrap();
    std::fs::create_dir_all(&dirs.state).unwrap();
    std::fs::create_dir_all(&dirs.runtime).unwrap();
    let mut binaries = BTreeMap::new();
    binaries.insert(v83, PathBuf::from("/usr/bin/true"));
    binaries.insert(v82, PathBuf::from("/usr/bin/true"));
    let mut mgr = PhpManager::new(
        spawner,
        FakeClock,
        FakeProbe::always_ok(),
        dirs,
        ActivePortBinder::new(),
        1234,
        binaries,
    );

    mgr.ensure(v83).await.unwrap();
    mgr.ensure(v82).await.unwrap();
    assert_eq!(mgr.snapshots().len(), 2);

    mgr.shutdown().await.unwrap();

    assert!(mgr.snapshots().is_empty());
    let kills_now = kills.lock().await;
    assert!(
        kills_now.iter().filter(|k| **k == KillSignal::Term).count() >= 2,
        "expected a SIGTERM per pool, got {kills_now:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn snapshots_report_failed_when_master_exited() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 401,
        behavior: ChildBehavior::LivesButTryWaitReportsExited(ExitReason::Code(1)),
    }]);
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);

    mgr.ensure(v).await.unwrap();
    let snaps = mgr.snapshots();
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].state, PoolRunState::Failed);
    assert_eq!(snaps[0].pid, None);
    assert!(snaps[0].listen.is_some());
}

/// A second ensure on a still-live pool must reuse the cached listen and not
/// pull a second spawn plan; only one plan is provided.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_is_idempotent_without_respawning() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 501,
        behavior: ChildBehavior::Lives,
    }]);
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);

    let first = mgr.ensure(v).await.unwrap();
    let second = mgr.ensure(v).await.unwrap();
    assert_eq!(first, second);
    let snaps = mgr.snapshots();
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].pid, Some(501));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn stop_then_snapshot_is_empty() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 601,
        behavior: ChildBehavior::LivesUntilKilled,
    }]);
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);

    mgr.ensure(v).await.unwrap();
    mgr.stop(v).await.unwrap();
    assert!(mgr.snapshots().is_empty());
    mgr.stop(v).await.unwrap();
}
