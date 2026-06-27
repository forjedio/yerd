//! End-to-end driver tests for [`ServiceManager`] with fakes for
//! `ProcessSpawner`, `Clock`, `ReadinessProbe`, and `ChildHandle`. Verifies that
//! `ServiceManager::ensure` drives the shared supervisor through the happy path,
//! crash + recovery, permanent failure, clean stop / shutdown, snapshots, and
//! the port pre-flight.
//!
//! Mirrors `yerd_php::tests::supervisor_states`. Stays fakes-only (no real
//! database binaries, no real sockets) so it passes on every CI target, since the SQL
//! engines are not installed on the test hosts. Happy-path coverage uses Redis
//! because it needs no datadir-init binary; the SQL engines' init seam is
//! covered by the in-file unit tests in `manager.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::Mutex;

use yerd_platform::{ActivePortBinder, PlatformDirs};
use yerd_services::version;
use yerd_services::{
    ReadinessProbe, Service, ServiceError, ServiceManager, ServiceRunState, ServiceVersion,
};
use yerd_supervise::supervisor::{KillSignal, StopProtocol, SupervisorPolicy};
use yerd_supervise::{ChildHandle, Clock, ExitReason, Listen, ProcessSpawner};

// ─── Fakes ──────────────────────────────────────────────────────────

/// Programmable child outcome.
#[derive(Clone)]
enum ChildBehavior {
    /// `wait()` resolves immediately with this exit reason.
    Crashes(ExitReason),
    /// `wait()` blocks forever (until killed); `try_wait()` reports alive.
    Lives,
    /// `wait()` blocks forever; `try_wait()` reports the child already exited.
    /// Used to drive a `Running` instance that `snapshots()` then sees as dead.
    LivesButTryWaitDead,
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
        let guard = self.behavior.try_lock().ok();
        match guard.as_deref() {
            Some(ChildBehavior::Crashes(r)) => Ok(Some(*r)),
            Some(ChildBehavior::LivesButTryWaitDead) => Ok(Some(ExitReason::Code(1))),
            _ => Ok(None),
        }
    }

    async fn wait(&mut self) -> Result<ExitReason, io::Error> {
        loop {
            let behavior = self.behavior.lock().await.clone();
            match behavior {
                ChildBehavior::Crashes(r) => return Ok(r),
                ChildBehavior::Lives | ChildBehavior::LivesButTryWaitDead => {
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

/// Programmable readiness probe. Each call pulls the next outcome from the queue;
/// when the queue empties, returns the `tail` outcome forever.
struct FakeProbe {
    tail: Result<(), io::ErrorKind>,
    /// Delay before producing an outcome. Used to make a crashing child's
    /// immediate `wait()` deterministically win the readiness race (the probe
    /// only wins for a child that stays alive).
    delay: std::time::Duration,
}

impl FakeProbe {
    fn always_ok() -> Self {
        Self {
            tail: Ok(()),
            delay: std::time::Duration::ZERO,
        }
    }

    /// Succeeds, but only after `delay`, so an already-crashed child wins the
    /// `select!` race while a live child is still reported ready.
    fn delayed_ok() -> Self {
        Self {
            tail: Ok(()),
            delay: std::time::Duration::from_millis(50),
        }
    }

    fn always_refused() -> Self {
        Self {
            tail: Err(io::ErrorKind::ConnectionRefused),
            delay: std::time::Duration::ZERO,
        }
    }
}

#[async_trait]
impl ReadinessProbe for FakeProbe {
    async fn probe(&self, _service: Service, _listen: &Listen) -> Result<(), io::Error> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        match self.tail {
            Ok(()) => Ok(()),
            Err(kind) => Err(io::Error::from(kind)),
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────

fn dirs_in(tmp: &std::path::Path) -> PlatformDirs {
    PlatformDirs {
        config: tmp.join("config"),
        data: tmp.join("data"),
        state: tmp.join("state"),
        cache: tmp.join("cache"),
        runtime: tmp.join("run"),
    }
}

/// Place a (dummy) server binary on disk so `ensure`'s `binary.is_file()` gate
/// passes. The fake spawner never actually executes it.
fn install_server_binary(dirs: &PlatformDirs, service: Service, v: &ServiceVersion) {
    let path = version::server_path(dirs, service, v);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"#!/bin/sh\n").unwrap();
}

/// Grab a currently-free loopback port (bind to 0, read it, release it). A small
/// TOCTOU window remains, acceptable for tests that never re-bind it.
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    l.local_addr().unwrap().port()
}

fn redis_version() -> ServiceVersion {
    ServiceVersion::from_str("8").unwrap()
}

type Mgr = ServiceManager<FakeSpawner, FakeClock, FakeProbe>;

fn make_manager(dirs: PlatformDirs, spawner: FakeSpawner, probe: FakeProbe) -> Mgr {
    ServiceManager::new(spawner, FakeClock, probe, dirs, ActivePortBinder::new())
}

// ─── Tests ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_happy_path_returns_loopback_listen() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let v = redis_version();
    install_server_binary(&dirs, Service::Redis, &v);
    let port = free_port();

    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 101,
        behavior: ChildBehavior::Lives,
    }]);
    let mut mgr = make_manager(dirs, spawner, FakeProbe::always_ok());

    let listen = mgr.ensure(Service::Redis, v.clone(), port).await.unwrap();
    match listen {
        Listen::TcpLoopback(addr) => {
            assert_eq!(addr, SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port));
        }
        Listen::UnixSocket(_) => panic!("services always listen on TCP loopback"),
    }

    let again = mgr.ensure(Service::Redis, v, port).await.unwrap();
    assert!(matches!(again, Listen::TcpLoopback(_)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_unknown_version_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let v = redis_version();
    let spawner = FakeSpawner::new(vec![]);
    let mut mgr = make_manager(dirs, spawner, FakeProbe::always_ok());

    let err = mgr
        .ensure(Service::Redis, v.clone(), free_port())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ServiceError::VersionNotInstalled { service: Service::Redis, ref version } if *version == v
        ),
        "got: {err:?}"
    );
}

/// A delayed-ok probe lets the first child's crash win the readiness race, then
/// reports the second (live) child ready, making recovery deterministic.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_recovers_after_one_crash() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let v = redis_version();
    install_server_binary(&dirs, Service::Redis, &v);

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
    let mut mgr = make_manager(dirs, spawner, FakeProbe::delayed_ok());

    let listen = mgr.ensure(Service::Redis, v, free_port()).await.unwrap();
    assert!(matches!(listen, Listen::TcpLoopback(_)));

    let snaps = mgr.snapshots();
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].state, ServiceRunState::Running);
    assert_eq!(snaps[0].pid, Some(102));
}

/// Provides more crashing children than the restart budget so a counting bug
/// cannot infinite-loop the test, and a refused probe that must not win the race
/// against the crashing child's exit.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_surfaces_permanent_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let v = redis_version();
    install_server_binary(&dirs, Service::Redis, &v);

    let max = SupervisorPolicy::database().max_restart_attempts;
    let plans: Vec<SpawnPlan> = (0..=max + 2)
        .map(|i| SpawnPlan {
            pid: 100 + i,
            behavior: ChildBehavior::Crashes(ExitReason::Code(1)),
        })
        .collect();
    let spawner = FakeSpawner::new(plans);
    let mut mgr = make_manager(dirs, spawner, FakeProbe::always_refused());

    let err = mgr
        .ensure(Service::Redis, v, free_port())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ServiceError::PermanentFailure {
                service: Service::Redis,
                ..
            }
        ),
        "got: {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn stop_kills_running_instance() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let v = redis_version();
    install_server_binary(&dirs, Service::Redis, &v);

    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 101,
        behavior: ChildBehavior::LivesUntilKilled,
    }]);
    let kills = spawner.kills_handle();
    let mut mgr = make_manager(dirs, spawner, FakeProbe::always_ok());

    mgr.ensure(Service::Redis, v, free_port()).await.unwrap();
    mgr.stop(Service::Redis).await.unwrap();

    let kills_now = kills.lock().await;
    assert!(
        kills_now.contains(&KillSignal::Term),
        "expected a SIGTERM, got {kills_now:?}"
    );
    drop(kills_now);
    assert!(mgr.snapshots().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn stop_on_unmanaged_service_is_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let spawner = FakeSpawner::new(vec![]);
    let mut mgr = make_manager(dirs, spawner, FakeProbe::always_ok());
    mgr.stop(Service::Redis).await.unwrap();
    mgr.shutdown().await.unwrap();
    assert!(mgr.snapshots().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn shutdown_stops_every_instance() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let v = redis_version();
    install_server_binary(&dirs, Service::Redis, &v);

    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 101,
        behavior: ChildBehavior::LivesUntilKilled,
    }]);
    let kills = spawner.kills_handle();
    let mut mgr = make_manager(dirs, spawner, FakeProbe::always_ok());
    mgr.ensure(Service::Redis, v, free_port()).await.unwrap();

    mgr.shutdown().await.unwrap();
    assert!(mgr.snapshots().is_empty());
    assert!(kills.lock().await.contains(&KillSignal::Term));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn restart_stops_then_starts_a_fresh_child() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let v = redis_version();
    install_server_binary(&dirs, Service::Redis, &v);
    let port = free_port();

    let spawner = FakeSpawner::new(vec![
        SpawnPlan {
            pid: 101,
            behavior: ChildBehavior::LivesUntilKilled,
        },
        SpawnPlan {
            pid: 202,
            behavior: ChildBehavior::Lives,
        },
    ]);
    let mut mgr = make_manager(dirs, spawner, FakeProbe::always_ok());

    mgr.ensure(Service::Redis, v.clone(), port).await.unwrap();
    let listen = mgr.restart(Service::Redis, v, port).await.unwrap();
    assert!(matches!(listen, Listen::TcpLoopback(_)));

    let snaps = mgr.snapshots();
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].pid, Some(202));
    assert_eq!(snaps[0].state, ServiceRunState::Running);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn snapshots_empty_when_nothing_started() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let spawner = FakeSpawner::new(vec![]);
    let mut mgr = make_manager(dirs, spawner, FakeProbe::always_ok());
    assert!(mgr.snapshots().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn snapshots_report_running_with_pid_and_listen() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let v = redis_version();
    install_server_binary(&dirs, Service::Redis, &v);

    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 101,
        behavior: ChildBehavior::Lives,
    }]);
    let mut mgr = make_manager(dirs, spawner, FakeProbe::always_ok());
    mgr.ensure(Service::Redis, v.clone(), free_port())
        .await
        .unwrap();

    let snaps = mgr.snapshots();
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].service, Service::Redis);
    assert_eq!(snaps[0].version, v);
    assert_eq!(snaps[0].state, ServiceRunState::Running);
    assert_eq!(snaps[0].pid, Some(101));
    assert!(snaps[0].listen.is_some());
}

/// The child passes the readiness probe (so the instance reaches Running), but
/// `try_wait` then reports it has already exited.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn snapshots_report_failed_when_child_has_died() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let v = redis_version();
    install_server_binary(&dirs, Service::Redis, &v);

    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 101,
        behavior: ChildBehavior::LivesButTryWaitDead,
    }]);
    let mut mgr = make_manager(dirs, spawner, FakeProbe::always_ok());
    mgr.ensure(Service::Redis, v, free_port()).await.unwrap();

    let snaps = mgr.snapshots();
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].state, ServiceRunState::Failed);
    assert_eq!(snaps[0].pid, None);
    assert!(snaps[0].listen.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_reports_port_conflict_as_port_in_use() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path());
    let v = redis_version();
    install_server_binary(&dirs, Service::Redis, &v);

    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();

    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 101,
        behavior: ChildBehavior::Lives,
    }]);
    let mut mgr = make_manager(dirs, spawner, FakeProbe::always_ok());

    let err = mgr.ensure(Service::Redis, v, port).await.unwrap_err();
    assert!(
        matches!(err, ServiceError::PortInUse { service: Service::Redis, port: p } if p == port),
        "got: {err:?}"
    );
    drop(listener);
}
