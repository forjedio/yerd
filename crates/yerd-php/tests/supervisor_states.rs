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
use yerd_php::pure::supervisor::{KillSignal, MAX_RESTART_ATTEMPTS};
use yerd_php::{
    ChildHandle, Clock, ExitReason, HealthProbe, Listen, PhpError, PhpManager, ProcessSpawner,
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
        // Non-blocking: only resolves if Crashes (already exited).
        let guard = self.behavior.try_lock().ok();
        match guard.as_deref() {
            Some(ChildBehavior::Crashes(r)) => Ok(Some(*r)),
            _ => Ok(None),
        }
    }

    async fn wait(&mut self) -> Result<ExitReason, io::Error> {
        loop {
            let behavior = self.behavior.lock().await.clone();
            match behavior {
                ChildBehavior::Crashes(r) => return Ok(r),
                ChildBehavior::Lives => {
                    // Pending forever.
                    std::future::pending::<()>().await;
                }
                ChildBehavior::LivesUntilKilled => {
                    self.killed_notify.notified().await;
                    // After kill, behavior is updated by kill().
                }
            }
        }
    }

    async fn kill(&mut self, signal: KillSignal) -> Result<(), io::Error> {
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
        // Sync trait — block on the mutex briefly using try_lock.
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
    // Make sure the config dir exists so atomic_write succeeds.
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
        Listen::TcpLoopback(_) => { /* Windows path, fine */ }
        _ => panic!("unexpected Listen variant"),
    }

    // Idempotent: second ensure returns immediately without re-spawning.
    let _ = mgr.ensure(v).await.unwrap();
    // Drop the manager so its child handles get dropped cleanly.
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
    // Need MAX_RESTART_ATTEMPTS+1 crashing children: attempts 1..=MAX
    // each crash; on the MAX-th BackoffElapsed the supervisor emits
    // PermanentFailure. Provide a few extra plans so a counting bug
    // doesn't infinite-loop the test.
    let plans: Vec<SpawnPlan> = (0..=MAX_RESTART_ATTEMPTS + 2)
        .map(|i| SpawnPlan {
            pid: 100 + i,
            behavior: ChildBehavior::Crashes(ExitReason::Code(1)),
        })
        .collect();
    let spawner = FakeSpawner::new(plans);
    // Probe must NOT win the race against `wait()` — use refused so the
    // crashing child wins.
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

// Sanity: silences `dead_code` warning for `FakeSpawner::spawn_count` if a
// future test wants to assert spawn counts directly.
#[allow(dead_code)]
async fn _spawn_count_helper(s: &FakeSpawner) -> usize {
    s.spawn_count().await
}
