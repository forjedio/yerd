//! End-to-end driver tests with fakes for `ProcessSpawner`, `Clock`,
//! `HealthProbe`, and `ChildHandle`. Verifies that `PhpManager::ensure`
//! drives the supervisor through the happy path, crash + recovery,
//! permanent failure, and clean stop.
//!
//! Live FPM coverage lands in `bin/yerdd`'s integration suite; this test stays
//! fakes-only for the process/clock/probe edges. It still uses the *real*
//! `ActivePortBinder`, which every supported OS now implements: Unix ignores it
//! (the planner takes the Unix-socket path) and Windows binds a loopback port
//! for the php-cgi `-b` address, so the file runs on both.

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
    ProcessSpawner, WORKERS_PER_VERSION,
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
    spawn_count: Arc<Mutex<usize>>,
    spawned_args: Arc<Mutex<Vec<Vec<String>>>>,
    last_kills: Arc<Mutex<Vec<KillSignal>>>,
}

impl FakeSpawner {
    fn new(plans: Vec<SpawnPlan>) -> Self {
        Self {
            plans: Mutex::new(plans.into()),
            spawn_count: Arc::new(Mutex::new(0)),
            spawned_args: Arc::new(Mutex::new(Vec::new())),
            last_kills: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Shared spawn counter, still readable once the spawner has been moved
    /// into the manager.
    fn count_handle(&self) -> Arc<Mutex<usize>> {
        Arc::clone(&self.spawn_count)
    }

    /// Argv of every spawned command, in spawn order. Windows bakes the planned
    /// address into `-b <host:port>`, which is how a test sees which port a
    /// worker was started on.
    #[cfg(windows)]
    fn args_handle(&self) -> Arc<Mutex<Vec<Vec<String>>>> {
        Arc::clone(&self.spawned_args)
    }

    fn kills_handle(&self) -> Arc<Mutex<Vec<KillSignal>>> {
        Arc::clone(&self.last_kills)
    }
}

impl ProcessSpawner for FakeSpawner {
    type Child = FakeChild;

    fn spawn(&self, cmd: std::process::Command) -> Result<FakeChild, io::Error> {
        let mut plans = self
            .plans
            .try_lock()
            .map_err(|_| io::Error::other("spawn: plans lock contended"))?;
        let mut counter = self
            .spawn_count
            .try_lock()
            .map_err(|_| io::Error::other("spawn: count lock contended"))?;
        let mut args = self
            .spawned_args
            .try_lock()
            .map_err(|_| io::Error::other("spawn: args lock contended"))?;
        let plan = plans
            .pop_front()
            .ok_or_else(|| io::Error::other("spawn: no more plans"))?;
        *counter += 1;
        args.push(
            cmd.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
        );
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
    /// When set, every probe is refused until the shared spawn counter reaches
    /// the threshold and succeeds from then on, overriding `sequence`/`tail`.
    /// Keying off spawns rather than probe calls keeps a test deterministic: the
    /// driver races the probe against the child's exit, so how many probes a
    /// failing attempt actually issues is not fixed.
    ok_after_spawns: Option<(Arc<Mutex<usize>>, usize)>,
}

impl FakeProbe {
    fn always_ok() -> Self {
        Self {
            sequence: Mutex::new(std::collections::VecDeque::new()),
            tail: Ok(()),
            ok_after_spawns: None,
        }
    }

    fn always_refused() -> Self {
        Self {
            sequence: Mutex::new(std::collections::VecDeque::new()),
            tail: Err(io::ErrorKind::ConnectionRefused),
            ok_after_spawns: None,
        }
    }

    /// Refuse until `spawns` has reached `threshold`, then accept.
    #[cfg(windows)]
    fn ok_after_spawns(spawns: Arc<Mutex<usize>>, threshold: usize) -> Self {
        Self {
            sequence: Mutex::new(std::collections::VecDeque::new()),
            tail: Ok(()),
            ok_after_spawns: Some((spawns, threshold)),
        }
    }
}

#[async_trait]
impl HealthProbe for FakeProbe {
    async fn probe(&self, _listen: &Listen) -> Result<(), io::Error> {
        if let Some((spawns, threshold)) = &self.ok_after_spawns {
            let spawned = *spawns.lock().await;
            if spawned >= *threshold {
                return Ok(());
            }
            return Err(io::Error::from(io::ErrorKind::ConnectionRefused));
        }
        let mut seq = self.sequence.lock().await;
        let outcome = seq.pop_front().unwrap_or(self.tail);
        match outcome {
            Ok(()) => Ok(()),
            Err(kind) => Err(io::Error::from(kind)),
        }
    }
}

/// A private directory tree per call, under the platform temp dir.
///
/// These were once a fixed `/tmp/yerd-test`, shared by every test in this file.
/// That survived while each test drove a single spawn, but a Windows pool now
/// renders one `zz-yerd.ini` per worker, so `cargo test` running these in
/// parallel had several tests writing the same file and CI failed with
/// `PermissionDenied`. The readback tests below already used `tempfile`; this
/// brings the shared helper in line with them.
fn make_dirs() -> PlatformDirs {
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pid = std::process::id();
    let root = std::env::temp_dir().join(format!("yerd-test-{pid}-{n}"));
    PlatformDirs {
        config: root.join("cfg"),
        data: root.join("data"),
        state: root.join("state"),
        cache: root.join("cache"),
        runtime: root.join("run"),
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

/// One spawn plan per worker of a version, with consecutive PIDs from
/// `first_pid`. Sized off `WORKERS_PER_VERSION` so a test that drives a whole
/// rotation holds at both N=1 and N=4.
fn worker_plans(first_pid: u32, behavior: &ChildBehavior) -> Vec<SpawnPlan> {
    (0..WORKERS_PER_VERSION)
        .map(|i| SpawnPlan {
            pid: first_pid + u32::try_from(i).unwrap(),
            behavior: behavior.clone(),
        })
        .collect()
}

/// The `-b <host:port>` address a php-cgi command was spawned with.
#[cfg(windows)]
fn bind_addr(args: &[String]) -> Option<String> {
    let at = args.iter().position(|a| a == "-b")?;
    args.get(at + 1).cloned()
}

// ─── Tests ──────────────────────────────────────────────────────────

/// A full set of spawn plans, because the second `ensure` rotates onto worker 1
/// wherever `WORKERS_PER_VERSION` is more than one.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_happy_path_returns_listen() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(worker_plans(101, &ChildBehavior::Lives));
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);

    let listen = mgr.ensure(v).await.unwrap();
    match listen {
        Listen::UnixSocket(p) => assert!(p.to_string_lossy().contains("fpm-8.3-1234.sock")),
        Listen::TcpLoopback(_) => {}
    }

    let _ = mgr.ensure(v).await.unwrap();
}

/// Each `ensure` hands out the next worker, and the cursor wraps back to worker
/// 0 on the `WORKERS_PER_VERSION + 1`-th call without spawning again.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_rotates_across_workers() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(worker_plans(701, &ChildBehavior::Lives));
    let spawns = spawner.count_handle();
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);

    let mut listens = Vec::new();
    for _ in 0..=WORKERS_PER_VERSION {
        listens.push(mgr.ensure(v).await.unwrap());
    }

    assert_eq!(*spawns.lock().await, WORKERS_PER_VERSION);
    assert_eq!(
        listens[0], listens[WORKERS_PER_VERSION],
        "the cursor must wrap back onto worker 0"
    );
    for a in 0..WORKERS_PER_VERSION {
        for b in (a + 1)..WORKERS_PER_VERSION {
            assert_ne!(
                listens[a], listens[b],
                "workers {a} and {b} must not share an address"
            );
        }
    }
    assert_eq!(
        mgr.snapshots().len(),
        1,
        "one row per version, not per worker"
    );
}

/// A version whose very first `ensure` fails must leave `pools` untouched: the
/// daemon renders "no pool" as `Stopped`, whereas an empty pool would read as
/// `Failed` and start the snapshot-driven restart loops against a version that
/// never ran.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn failed_first_ensure_leaves_no_pool() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![]);
    let mut mgr = make_manager(spawner, FakeProbe::always_refused(), v);

    assert!(mgr.ensure(v).await.is_err());
    assert!(mgr.snapshots().is_empty());
}

/// Both hosts write the CA bundle, in different files and different syntax, so
/// the assertions are cfg-branched rather than gated. Unix renders an FPM pool
/// file under `config/`; Windows renders the php-cgi supplemental
/// `zz-yerd.ini` under `state/php/fpm-<v>-<id>/`, with the path double-quoted
/// and backslash-doubled by `sanitize_quoted_ca_bundle_path`.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn set_ca_bundle_is_rendered_into_pool_config() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 101,
        behavior: ChildBehavior::Lives,
    }]);
    let tmp = tempfile::tempdir().unwrap();
    let dirs = PlatformDirs {
        config: tmp.path().join("cfg"),
        data: tmp.path().join("data"),
        state: tmp.path().join("state"),
        cache: tmp.path().join("cache"),
        runtime: tmp.path().join("run"),
    };
    std::fs::create_dir_all(&dirs.config).unwrap();
    std::fs::create_dir_all(&dirs.state).unwrap();
    std::fs::create_dir_all(&dirs.runtime).unwrap();
    let bundle = dirs.data.join("cacert.pem");
    let mut mgr = PhpManager::new(
        spawner,
        FakeClock,
        FakeProbe::always_ok(),
        dirs.clone(),
        ActivePortBinder::new(),
        5150,
        binaries_with(v),
    );
    mgr.set_ca_bundle(Some(bundle.clone()));

    mgr.ensure(v).await.unwrap();

    #[cfg(not(windows))]
    {
        let cfg_path = dirs.config.join("php-fpm-8.3-5150.conf");
        let on_disk = std::fs::read_to_string(&cfg_path).unwrap();
        assert!(
            on_disk.contains(&format!(
                "php_admin_value[openssl.cafile] = {}\n",
                bundle.display()
            )),
            "got: {on_disk}"
        );
        assert!(
            on_disk.contains(&format!(
                "php_admin_value[curl.cainfo] = {}\n",
                bundle.display()
            )),
            "got: {on_disk}"
        );
    }
    #[cfg(windows)]
    {
        let cfg_path = dirs
            .state
            .join("php")
            .join("fpm-8.3-5150")
            .join("zz-yerd.ini");
        let on_disk = std::fs::read_to_string(&cfg_path).unwrap();
        let quoted = yerd_core::php_settings::sanitize_quoted_ca_bundle_path(&bundle).unwrap();
        assert!(
            on_disk.contains(&format!("openssl.cafile = \"{quoted}\"\n")),
            "got: {on_disk}"
        );
        assert!(
            on_disk.contains(&format!("curl.cainfo = \"{quoted}\"\n")),
            "got: {on_disk}"
        );
    }
}

/// Unix-only: `pm.max_children` is an FPM pool directive with no Windows
/// counterpart. `pure::fpm_conf::render_win_ini` emits no `pm.*` line at all,
/// and `build_cmd`'s Windows arm clears `PHP_FCGI_CHILDREN` because php-cgi has
/// no fork-based worker pool, which is why the pool-size setting is hidden and
/// `yerd php pool set` is refused on Windows. Gated rather than weakened: the
/// Unix assertions below stay exactly as they were.
#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn set_pool_overrides_is_rendered_into_pool_config() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 101,
        behavior: ChildBehavior::Lives,
    }]);
    let tmp = tempfile::tempdir().unwrap();
    let dirs = PlatformDirs {
        config: tmp.path().join("cfg"),
        data: tmp.path().join("data"),
        state: tmp.path().join("state"),
        cache: tmp.path().join("cache"),
        runtime: tmp.path().join("run"),
    };
    std::fs::create_dir_all(&dirs.config).unwrap();
    std::fs::create_dir_all(&dirs.state).unwrap();
    std::fs::create_dir_all(&dirs.runtime).unwrap();
    let mut mgr = PhpManager::new(
        spawner,
        FakeClock,
        FakeProbe::always_ok(),
        dirs.clone(),
        ActivePortBinder::new(),
        5151,
        binaries_with(v),
    );
    mgr.set_pool_overrides(BTreeMap::from([(
        v,
        BTreeMap::from([("max_children".to_owned(), "32".to_owned())]),
    )]));

    mgr.ensure(v).await.unwrap();

    let on_disk = std::fs::read_to_string(dirs.config.join("php-fpm-8.3-5151.conf")).unwrap();
    assert!(on_disk.contains("pm.max_children = 32\n"), "got: {on_disk}");
    assert!(!on_disk.contains("pm.max_children = 16"), "got: {on_disk}");
}

/// An override that no longer validates leaves the built-in default alone
/// rather than breaking the pool: `override_max_children` returns `None` and
/// `ensure` never touches `cfg.max_children`.
///
/// Unix-only for the same reason as
/// `set_pool_overrides_is_rendered_into_pool_config`: Windows renders no
/// `pm.*` directive, so there is no default to fall back to on that host.
#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn invalid_pool_override_falls_back_to_the_default() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(vec![SpawnPlan {
        pid: 101,
        behavior: ChildBehavior::Lives,
    }]);
    let tmp = tempfile::tempdir().unwrap();
    let dirs = PlatformDirs {
        config: tmp.path().join("cfg"),
        data: tmp.path().join("data"),
        state: tmp.path().join("state"),
        cache: tmp.path().join("cache"),
        runtime: tmp.path().join("run"),
    };
    std::fs::create_dir_all(&dirs.config).unwrap();
    std::fs::create_dir_all(&dirs.state).unwrap();
    std::fs::create_dir_all(&dirs.runtime).unwrap();
    let mut mgr = PhpManager::new(
        spawner,
        FakeClock,
        FakeProbe::always_ok(),
        dirs.clone(),
        ActivePortBinder::new(),
        5152,
        binaries_with(v),
    );
    mgr.set_pool_overrides(BTreeMap::from([(
        v,
        BTreeMap::from([("max_children".to_owned(), "0".to_owned())]),
    )]));

    mgr.ensure(v).await.unwrap();

    let on_disk = std::fs::read_to_string(dirs.config.join("php-fpm-8.3-5152.conf")).unwrap();
    assert!(on_disk.contains("pm.max_children = 16\n"), "got: {on_disk}");
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

/// A fully populated pool still reports exactly one row, carrying the first
/// live worker's PID.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn snapshots_report_running_pool_with_pid() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(worker_plans(101, &ChildBehavior::Lives));
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);
    for _ in 0..WORKERS_PER_VERSION {
        mgr.ensure(v).await.unwrap();
    }

    let snaps = mgr.snapshots();
    assert_eq!(snaps.len(), 1, "one row per version, not per worker");
    assert_eq!(snaps[0].version, v);
    assert_eq!(snaps[0].state, PoolRunState::Running);
    assert_eq!(snaps[0].pid, Some(101), "the first live worker's pid");
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

/// R2: the port baked into a worker's config can go stale between
/// `AllocatedListen::plan` dropping its probe listener and php-cgi binding it,
/// so a failed start is replanned onto a fresh port and retried exactly once.
/// The probe refuses everything until the retry's spawn, so the first drive
/// cannot succeed on the original port however its `select!` races resolve.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_replans_the_port_after_a_failed_start() {
    let v = PhpVersion::new(8, 3);
    let max = usize::try_from(SupervisorPolicy::fpm().max_restart_attempts).unwrap();
    let mut plans: Vec<SpawnPlan> = (0..max)
        .map(|i| SpawnPlan {
            pid: 801 + u32::try_from(i).unwrap(),
            behavior: ChildBehavior::Crashes(ExitReason::Code(1)),
        })
        .collect();
    plans.push(SpawnPlan {
        pid: 901,
        behavior: ChildBehavior::Lives,
    });
    let spawner = FakeSpawner::new(plans);
    let spawns = spawner.count_handle();
    let args = spawner.args_handle();
    let probe = FakeProbe::ok_after_spawns(spawner.count_handle(), max + 1);
    let mut mgr = make_manager(spawner, probe, v);

    let listen = mgr.ensure(v).await.unwrap();

    assert_eq!(
        *spawns.lock().await,
        max + 1,
        "one whole drive, then one replanned retry"
    );
    let bound: Vec<String> = args
        .lock()
        .await
        .iter()
        .filter_map(|a| bind_addr(a))
        .collect();
    assert_eq!(bound.len(), max + 1, "every php-cgi spawn carries -b");
    assert!(
        bound[..max].iter().all(|a| *a == bound[0]),
        "the first drive retries on one port: {bound:?}"
    );
    assert_ne!(bound[max], bound[0], "the retry must plan a fresh port");
    match listen {
        Listen::TcpLoopback(addr) => assert_eq!(addr.to_string(), bound[max]),
        other @ Listen::UnixSocket(_) => panic!("expected TcpLoopback, got {other:?}"),
    }
}

/// Exactly one drive's worth of spawn plans. Windows replans and spends a
/// second drive, which finds the plan queue empty and surfaces `Spawn`;
/// everywhere else there is no retry, so the drive's own `PermanentFailure`
/// reaches the caller off the same plan count.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn failed_start_is_replanned_only_on_windows() {
    let v = PhpVersion::new(8, 3);
    let max = usize::try_from(SupervisorPolicy::fpm().max_restart_attempts).unwrap();
    let plans: Vec<SpawnPlan> = (0..max)
        .map(|i| SpawnPlan {
            pid: 851 + u32::try_from(i).unwrap(),
            behavior: ChildBehavior::Crashes(ExitReason::Code(1)),
        })
        .collect();
    let spawner = FakeSpawner::new(plans);
    let spawns = spawner.count_handle();
    let mut mgr = make_manager(spawner, FakeProbe::always_refused(), v);

    let err = mgr.ensure(v).await.unwrap_err();

    assert_eq!(*spawns.lock().await, max);
    #[cfg(windows)]
    assert!(matches!(err, PhpError::Spawn { .. }), "got: {err:?}");
    #[cfg(not(windows))]
    assert!(
        matches!(err, PhpError::PermanentFailure { .. }),
        "got: {err:?}"
    );
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

/// Every worker of the pool has died, so the version's single row is `Failed`
/// with no PID.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn snapshots_report_failed_when_master_exited() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(worker_plans(
        401,
        &ChildBehavior::LivesButTryWaitReportsExited(ExitReason::Code(1)),
    ));
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);

    for _ in 0..WORKERS_PER_VERSION {
        mgr.ensure(v).await.unwrap();
    }

    let snaps = mgr.snapshots();
    assert_eq!(snaps.len(), 1, "one row per version, not per worker");
    assert_eq!(snaps[0].state, PoolRunState::Failed);
    assert_eq!(snaps[0].pid, None);
    assert!(snaps[0].listen.is_some());
}

/// An `ensure` that lands on a still-live worker must reuse its cached listen
/// rather than pull another spawn plan. One full rotation puts the cursor back
/// on worker 0, so the `WORKERS_PER_VERSION + 1`-th call returns the first
/// call's address with no extra spawn. The single-row snapshot with the first
/// worker's PID is the per-version aggregation check.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ensure_is_idempotent_without_respawning() {
    let v = PhpVersion::new(8, 3);
    let spawner = FakeSpawner::new(worker_plans(501, &ChildBehavior::Lives));
    let spawns = spawner.count_handle();
    let mut mgr = make_manager(spawner, FakeProbe::always_ok(), v);

    let first = mgr.ensure(v).await.unwrap();
    let mut last = first.clone();
    for _ in 0..WORKERS_PER_VERSION {
        last = mgr.ensure(v).await.unwrap();
    }

    assert_eq!(first, last);
    assert_eq!(*spawns.lock().await, WORKERS_PER_VERSION);
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
