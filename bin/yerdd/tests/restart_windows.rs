//! Windows console-mode restart: spawn the real `yerdd.exe` against a tempdir,
//! capture its `boot_id`, send `RestartDaemon`, and prove a *different* process
//! comes back on the same pipe with a changed `boot_id` (the spawn-new-then-exit
//! handoff). Windows-only; the Unix restart is an in-place `exec` covered by the
//! lifecycle test's daemon.

#![cfg(windows)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use interprocess::local_socket::tokio::Stream as IpcStream;
use yerd_ipc::{read_message, write_message, FrameDecoder, Request, Response, DEFAULT_MAX_FRAME};
use yerd_platform::PlatformDirs;

/// Kills the daemon process tree on drop so a panicking test never orphans a
/// running `yerdd.exe`. Holds the original spawned child plus any successor pid
/// discovered from a `Status` response.
struct Reaper {
    original: std::process::Child,
    successor_pid: Option<u32>,
}

impl Drop for Reaper {
    fn drop(&mut self) {
        if let Some(pid) = self.successor_pid {
            taskkill(pid);
        }
        taskkill(self.original.id());
        let _ = self.original.kill();
        let _ = self.original.wait();
    }
}

/// `taskkill /PID <pid> /T /F` via the absolute System32 path (never PATH).
fn taskkill(pid: u32) {
    let taskkill = std::env::var_os("SystemRoot")
        .map_or_else(
            || std::path::PathBuf::from(r"C:\Windows"),
            std::path::PathBuf::from,
        )
        .join("System32")
        .join("taskkill.exe");
    let _ = Command::new(taskkill)
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
}

/// Client-side dirs whose `runtime` matches what the daemon derives from `TEMP`
/// (`std::env::temp_dir().join("yerd")`), so `daemon_pipe_name` produces the same
/// pipe the daemon bound.
fn client_dirs(temp: &Path, appdata: &Path, local: &Path) -> PlatformDirs {
    PlatformDirs {
        config: appdata.join("yerd"),
        data: local.join("yerd").join("data"),
        state: local.join("yerd").join("state"),
        cache: local.join("yerd").join("cache"),
        runtime: temp.join("yerd"),
    }
}

/// One `Status` exchange over the daemon pipe. `None` on any connect/transport
/// error (daemon not up yet, or mid-restart).
async fn status(dirs: &PlatformDirs) -> Option<yerd_ipc::StatusReport> {
    use interprocess::local_socket::traits::tokio::Stream as _;
    use interprocess::local_socket::{GenericNamespaced, ToNsName};

    let pipe = yerd_platform::daemon_pipe_name(dirs).ok()?;
    let name = pipe.to_ns_name::<GenericNamespaced>().ok()?;
    let stream = IpcStream::connect(name).await.ok()?;
    let (reader, writer) = stream.split();
    let mut reader = reader;
    let mut writer = writer;
    write_message(&mut writer, &Request::Status, DEFAULT_MAX_FRAME)
        .await
        .ok()?;
    let mut decoder = FrameDecoder::new();
    match read_message::<_, Response>(&mut reader, &mut decoder)
        .await
        .ok()??
    {
        Response::Status { report } => Some(*report),
        _ => None,
    }
}

/// Poll `Status` until the predicate returns `Some`, or the deadline elapses.
async fn poll<T>(
    dirs: &PlatformDirs,
    deadline: Duration,
    mut pred: impl FnMut(&yerd_ipc::StatusReport) -> Option<T>,
) -> Option<T> {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if let Some(report) = status(dirs).await {
            if let Some(v) = pred(&report) {
                return Some(v);
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    None
}

#[test]
/// Uses ephemeral-safe ports so the spawned daemon never fights over 80/443/53
/// with anything real on the host.
fn console_restart_spawns_successor_with_new_boot_id() {
    let root = tempfile::tempdir().unwrap();
    let temp = root.path().join("temp");
    let appdata = root.path().join("appdata");
    let local = root.path().join("local");
    for d in [&temp, &appdata, &local] {
        std::fs::create_dir_all(d).unwrap();
    }

    let (http, https) = free_ports();
    let mut cfg = yerd_config::Config::default();
    cfg.ports.http = http;
    cfg.ports.https = https;
    cfg.dns_port = 0;
    let cfg_dir = appdata.join("yerd");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("yerd.toml"), cfg.to_toml().unwrap()).unwrap();

    let exe = env!("CARGO_BIN_EXE_yerdd");
    let original = Command::new(exe)
        .env("APPDATA", &appdata)
        .env("LOCALAPPDATA", &local)
        .env("TEMP", &temp)
        .env("TMP", &temp)
        .spawn()
        .expect("spawn yerdd.exe");

    let mut reaper = Reaper {
        original,
        successor_pid: None,
    };

    let dirs = client_dirs(&temp, &appdata, &local);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        let (before_boot, before_pid) = poll(&dirs, Duration::from_secs(30), |r| {
            r.boot_id.map(|b| (b, r.daemon_pid))
        })
        .await
        .expect("daemon came up and reported a boot_id");
        reaper.successor_pid = Some(before_pid);

        status_restart(&dirs).await;

        let (after_boot, after_pid) = poll(&dirs, Duration::from_secs(45), |r| {
            r.boot_id
                .filter(|b| *b != before_boot)
                .map(|b| (b, r.daemon_pid))
        })
        .await
        .expect("successor daemon came up with a changed boot_id");
        reaper.successor_pid = Some(after_pid);

        assert_ne!(
            before_boot, after_boot,
            "boot_id must change across restart"
        );
        assert_ne!(
            before_pid, after_pid,
            "the successor is a distinct process (spawn-new-then-exit)"
        );
    });
}

/// Best-effort `RestartDaemon` (the daemon writes `Ok` and flushes before it
/// tears down, so this returns normally, but a transient error is fine too).
async fn status_restart(dirs: &PlatformDirs) {
    use interprocess::local_socket::traits::tokio::Stream as _;
    use interprocess::local_socket::{GenericNamespaced, ToNsName};

    let Ok(pipe) = yerd_platform::daemon_pipe_name(dirs) else {
        return;
    };
    let Ok(name) = pipe.to_ns_name::<GenericNamespaced>() else {
        return;
    };
    let Ok(stream) = IpcStream::connect(name).await else {
        return;
    };
    let (_reader, writer) = stream.split();
    let mut writer = writer;
    let _ = write_message(&mut writer, &Request::RestartDaemon, DEFAULT_MAX_FRAME).await;
}

/// Two distinct, currently-free, non-zero TCP ports.
fn free_ports() -> (u16, u16) {
    let a = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let b = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let pa = a.local_addr().unwrap().port();
    let pb = b.local_addr().unwrap().port();
    drop(a);
    drop(b);
    assert_ne!(pa, pb);
    (pa, pb)
}
