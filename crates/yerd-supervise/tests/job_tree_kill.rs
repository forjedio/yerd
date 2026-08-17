//! Windows Job Object orphan guard: no worker may outlive the daemon.
//!
//! Spawns a leader that provably starts a detached grandchild, then proves the
//! grandchild dies with the leader on both the explicit-`kill` path and the
//! drop (kill-on-close) path. This exercises `win32job`'s `KILL_ON_JOB_CLOSE`
//! reaping end to end, which is what replaces Unix process-group teardown on
//! Windows. Unix has its own `killpg` coverage; this file is Windows-only.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::Command as StdCommand;
use std::time::{Duration, Instant};

use yerd_supervise::{ChildHandle, KillSignal, ProcessSpawner, StopProtocol, TokioProcessSpawner};

/// A leader that starts a long-lived, detached `ping` grandchild (a real
/// orphan-risk: it outlives its parent unless the job reaps it), records the
/// grandchild PID to `pidfile`, then sleeps so the leader stays alive until we
/// kill or drop it. The grandchild joins the leader's job by inheritance.
fn leader_command(pidfile: &Path) -> StdCommand {
    let script = format!(
        "$p = Start-Process ping -ArgumentList '-n','120','127.0.0.1' -PassThru -WindowStyle Hidden; \
         Set-Content -LiteralPath '{}' -Value $p.Id; \
         Start-Sleep -Seconds 120",
        pidfile.display()
    );
    let mut cmd = StdCommand::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    cmd
}

/// Poll `pidfile` until the leader has written a parseable grandchild PID.
async fn read_pid(pidfile: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(s) = std::fs::read_to_string(pidfile) {
            if let Ok(pid) = s.trim().parse::<u32>() {
                return pid;
            }
        }
        assert!(Instant::now() < deadline, "grandchild PID never appeared");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Whether a `ping` process with `pid` is still running. `tasklist /FI` filters
/// to that PID, so a surviving row is the ping grandchild; no row means gone.
fn ping_alive(pid: u32) -> bool {
    let out = StdCommand::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("run tasklist");
    String::from_utf8_lossy(&out.stdout)
        .to_ascii_lowercase()
        .contains("ping")
}

/// Poll until the grandchild is gone, bounded.
async fn wait_until_dead(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if !ping_alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    !ping_alive(pid)
}

/// Best-effort cleanup so a failing assertion never leaks a 120s `ping`.
fn best_effort_kill(pid: u32) {
    let _ = StdCommand::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output();
}

#[tokio::test]
async fn kill_terminates_the_whole_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let pidfile = tmp.path().join("gc.pid");
    let mut leader = TokioProcessSpawner.spawn(leader_command(&pidfile)).unwrap();

    let grandchild = read_pid(&pidfile).await;
    assert!(ping_alive(grandchild), "grandchild should be running");

    leader
        .kill(KillSignal::Kill, StopProtocol::GroupTerm)
        .await
        .unwrap();
    let _ = leader.wait().await;

    let dead = wait_until_dead(grandchild).await;
    best_effort_kill(grandchild);
    assert!(
        dead,
        "grandchild {grandchild} must die with the killed leader (zero orphans)"
    );
}

#[tokio::test]
async fn drop_terminates_the_whole_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let pidfile = tmp.path().join("gc.pid");
    let leader = TokioProcessSpawner.spawn(leader_command(&pidfile)).unwrap();

    let grandchild = read_pid(&pidfile).await;
    assert!(ping_alive(grandchild), "grandchild should be running");

    // No explicit kill: dropping the child drops its job, whose kill-on-close
    // must reap the whole tree even though `kill_on_drop` alone covers only the
    // direct child. This is the daemon-crash guarantee.
    drop(leader);

    let dead = wait_until_dead(grandchild).await;
    best_effort_kill(grandchild);
    assert!(
        dead,
        "grandchild {grandchild} must die when the child's job is dropped (kill-on-close)"
    );
}

/// The graceful arm must still reap the tree. The leader never exits on its own,
/// so this drives the wait to its timeout and then the forced job teardown, which
/// is the path that must not leak an orphan.
#[tokio::test]
async fn graceful_term_forces_after_the_grace_and_reaps_the_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let pidfile = tmp.path().join("gc.pid");
    let mut leader = TokioProcessSpawner.spawn(leader_command(&pidfile)).unwrap();

    let grandchild = read_pid(&pidfile).await;
    assert!(ping_alive(grandchild), "grandchild should be running");

    let started = std::time::Instant::now();
    leader
        .kill(KillSignal::Term, StopProtocol::MasterInterrupt)
        .await
        .unwrap();
    let elapsed = started.elapsed();
    let _ = leader.wait().await;

    let dead = wait_until_dead(grandchild).await;
    best_effort_kill(grandchild);
    assert!(
        dead,
        "grandchild {grandchild} must die once the grace expires (zero orphans)"
    );
    assert!(
        elapsed >= yerd_supervise::pure::GRACEFUL_EXIT_WAIT,
        "a non-exiting child must be waited for the full grace, waited {elapsed:?}"
    );
}

/// A `Term` under `GroupTerm` must not wait at all: it is the force path, and
/// paying the grace there would slow every ordinary stop.
#[tokio::test]
async fn group_term_forces_immediately() {
    let tmp = tempfile::tempdir().unwrap();
    let pidfile = tmp.path().join("gc.pid");
    let mut leader = TokioProcessSpawner.spawn(leader_command(&pidfile)).unwrap();

    let grandchild = read_pid(&pidfile).await;
    assert!(ping_alive(grandchild), "grandchild should be running");

    let started = std::time::Instant::now();
    leader
        .kill(KillSignal::Term, StopProtocol::GroupTerm)
        .await
        .unwrap();
    let elapsed = started.elapsed();
    let _ = leader.wait().await;

    let dead = wait_until_dead(grandchild).await;
    best_effort_kill(grandchild);
    assert!(dead, "grandchild {grandchild} must die with the leader");
    assert!(
        elapsed < yerd_supervise::pure::GRACEFUL_EXIT_WAIT,
        "the force path must not pay the grace, waited {elapsed:?}"
    );
}
