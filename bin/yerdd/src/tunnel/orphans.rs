//! Reap tunnel processes (`cloudflared`, ngrok, zrok) a previous daemon run
//! left behind.
//!
//! The tunnel manager writes `{state}/tunnel/<key>.pid` for every spawn and
//! removes it on stop, so after a clean exit the directory holds no pidfiles. A
//! pidfile found at startup therefore names a connector orphaned by a daemon
//! crash, which would otherwise keep serving the tunnel against a stale config.
//! [`reap`] runs before the IPC server opens, so no pidfile of this run can be
//! mistaken for an orphan.
//!
//! A PID is only signalled when its command line belongs to one of Yerd's
//! tunnel children ([`is_owned_tunnel_process`]): every Yerd-spawned
//! `cloudflared` and ngrok carries a `{data}/tunnel/...` path through
//! `--config`, which guards against the PID having been reused by an unrelated
//! process. A zrok share's argv carries no Yerd path, so it is matched by its
//! exact share signature instead; a reused PID would have to be running that
//! same command for it to be signalled.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use yerd_platform::{PlatformDirs, ProcessTable};

/// How long an orphan gets to exit after SIGTERM before it is killed.
const REAP_GRACE: Duration = Duration::from_secs(5);

/// Poll interval while waiting for signalled orphans to exit.
const REAP_POLL: Duration = Duration::from_millis(100);

/// Executables that always run with a `--config` inside `{data}/tunnel`.
const CONFIGURED_AGENTS: [&str; 2] = ["cloudflared", "ngrok"];

/// The zrok executable, matched by its share signature instead of a path.
const ZROK: &str = "zrok2";

/// Whether `cmdline` is a tunnel process started by this Yerd install: a
/// `cloudflared` or `ngrok` executable (the word bounded by a path separator
/// or a space) referencing a path inside `tunnel_dir` (`{data}/tunnel`), or a
/// `zrok2` executable running Yerd's exact share command (`share public
/// http://127.0.0.1:<port>` with `--force-local`).
#[must_use]
pub fn is_owned_tunnel_process(cmdline: &str, tunnel_dir: &Path) -> bool {
    let dir = tunnel_dir.to_string_lossy();
    let dir = dir.trim_end_matches('/');
    let configured = !dir.is_empty()
        && CONFIGURED_AGENTS
            .iter()
            .any(|name| names_executable(cmdline, name))
        && cmdline.contains(&format!("{dir}/"));
    let zrok_share = names_executable(cmdline, ZROK)
        && cmdline.contains(" share public http://127.0.0.1:")
        && cmdline.contains("--force-local");
    configured || zrok_share
}

/// Whether `cmdline` contains `name` as a whole executable name.
fn names_executable(cmdline: &str, name: &str) -> bool {
    cmdline.match_indices(name).any(|(at, _)| {
        let before = cmdline.get(..at).and_then(|s| s.chars().next_back());
        let after = cmdline
            .get(at + name.len()..)
            .and_then(|s| s.chars().next());
        matches!(before, None | Some('/' | ' ')) && matches!(after, None | Some(' '))
    })
}

/// `{state}/tunnel`, where the tunnel manager keeps its pidfiles.
fn pid_dir(dirs: &PlatformDirs) -> PathBuf {
    dirs.state.join("tunnel")
}

/// Reap orphans through `table`: read every `{state}/tunnel/*.pid`, SIGTERM the
/// process group of each PID whose command line is an owned tunnel process,
/// wait up to `grace` for them to exit, SIGKILL any survivor whose command line
/// is still an owned tunnel process, and remove every pidfile. Blocking. Returns
/// the PIDs that were signalled.
///
/// The ownership recheck before SIGKILL guards against the PID being reused
/// during the grace window; without a process handle (no pidfd on macOS) a
/// reuse between that recheck and the signal cannot be ruled out entirely.
pub fn reap_with(table: &dyn ProcessTable, dirs: &PlatformDirs, grace: Duration) -> Vec<u32> {
    let tunnel_dir = super::install::tunnel_dir(dirs);
    let Ok(entries) = std::fs::read_dir(pid_dir(dirs)) else {
        return Vec::new();
    };
    let pidfiles: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "pid"))
        .collect();
    let mut owned = Vec::new();
    for path in &pidfiles {
        let pid = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok());
        if let Some(pid) = pid.filter(|pid| {
            yerd_platform::pure::process_signal::signal_target(*pid).is_some()
                && table
                    .command_line(*pid)
                    .is_some_and(|cmdline| is_owned_tunnel_process(&cmdline, &tunnel_dir))
        }) {
            if !owned.contains(&pid) {
                owned.push(pid);
            }
        }
        let _ = std::fs::remove_file(path);
    }
    for pid in &owned {
        tracing::info!(
            pid,
            "stopping orphaned tunnel process from a previous daemon run"
        );
        table.signal_group(*pid, false);
    }
    let deadline = Instant::now() + grace;
    while owned.iter().any(|pid| table.is_alive(*pid)) && Instant::now() < deadline {
        std::thread::sleep(REAP_POLL);
    }
    let still_owned = |pid: u32| {
        table
            .command_line(pid)
            .is_some_and(|cmdline| is_owned_tunnel_process(&cmdline, &tunnel_dir))
    };
    for pid in owned.iter().filter(|pid| still_owned(**pid)) {
        tracing::warn!(pid, "orphaned tunnel process ignored SIGTERM; killing it");
        table.signal_group(*pid, true);
    }
    owned
}

/// Reap orphans with the OS process table on the blocking pool, so the waits
/// and any `ps` subprocess never run on the async executor.
pub async fn reap(dirs: &PlatformDirs) {
    let dirs = dirs.clone();
    let reaped = tokio::task::spawn_blocking(move || {
        reap_with(&yerd_platform::ActiveProcessTable::new(), &dirs, REAP_GRACE)
    })
    .await;
    if let Err(e) = reaped {
        tracing::warn!(error = %e, "orphaned tunnel process reaping did not complete");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::test_support::state_in;
    use yerd_platform::{FakeProcessTable, RecordedSignal};

    const TUNNEL: &str = "/home/u/.local/share/yerd/tunnel";

    #[test]
    fn is_owned_tunnel_process_table() {
        let dir = Path::new(TUNNEL);
        let cases: &[(&str, bool)] = &[
            (
                "/home/u/.local/share/yerd/tunnel/bin/cloudflared tunnel --no-autoupdate \
                 --config /home/u/.local/share/yerd/tunnel/quick.yml --url http://127.0.0.1:1",
                true,
            ),
            (
                "/usr/local/bin/cloudflared --no-autoupdate --origincert \
                 /home/u/.local/share/yerd/tunnel/.cloudflared/cert.pem --config \
                 /home/u/.local/share/yerd/tunnel/named.yml tunnel run",
                true,
            ),
            ("cloudflared --config /home/u/.local/share/yerd/tunnel/named.yml", true),
            ("/usr/bin/vim /home/u/.local/share/yerd/tunnel/named.yml", false),
            ("/usr/local/bin/cloudflared tunnel --url http://localhost:8000", false),
            (
                "/usr/local/bin/cloudflared --config /home/other/.local/share/yerd/tunnel/named.yml",
                false,
            ),
            (
                "/usr/local/bin/cloudflared --config /home/u/.local/share/yerd/tunnel2/named.yml",
                false,
            ),
            (
                "/usr/bin/less /home/u/.local/share/yerd/tunnel/cloudflared.log",
                false,
            ),
            ("", false),
            (
                "/home/u/.local/share/yerd/tools/ngrok/ngrok http http://127.0.0.1:1 \
                 --config /home/u/.local/share/yerd/tunnel/ngrok.yml",
                true,
            ),
            ("/usr/local/bin/ngrok http http://127.0.0.1:1", false),
            (
                "/usr/local/bin/ngrokx http --config /home/u/.local/share/yerd/tunnel/ngrok.yml",
                false,
            ),
            (
                "/home/u/.local/share/yerd/tools/zrok/zrok2 share public http://127.0.0.1:4410 \
                 --headless --backend-mode proxy --force-local",
                true,
            ),
            ("/usr/local/bin/zrok2 share public localhost:3000", false),
            ("/usr/local/bin/zrok2 enable --headless tok", false),
            (
                "/usr/local/bin/zrok2 share public http://127.0.0.1:3000 --headless",
                false,
            ),
        ];
        for (cmdline, expected) in cases {
            assert_eq!(
                is_owned_tunnel_process(cmdline, dir),
                *expected,
                "{cmdline}"
            );
        }
    }

    #[test]
    fn is_owned_tunnel_process_handles_spaces_and_a_trailing_slash() {
        let dir = Path::new("/Users/u/Library/Application Support/yerd/tunnel/");
        assert!(is_owned_tunnel_process(
            "/Users/u/Library/Application Support/yerd/tunnel/bin/cloudflared --config \
             /Users/u/Library/Application Support/yerd/tunnel/quick.yml",
            dir
        ));
        assert!(!is_owned_tunnel_process(
            "cloudflared --config /tunnel/x",
            Path::new("/")
        ));
    }

    fn write_pidfile(dirs: &PlatformDirs, key: &str, contents: &str) -> PathBuf {
        let dir = pid_dir(dirs);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{key}.pid"));
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn owned_cmdline(dirs: &PlatformDirs) -> String {
        let tunnel = super::super::install::tunnel_dir(dirs);
        format!(
            "{}/bin/cloudflared tunnel --config {}/quick.yml",
            tunnel.display(),
            tunnel.display()
        )
    }

    #[test]
    fn reap_signals_only_owned_pids_and_removes_every_pidfile() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = state_in(tmp.path()).dirs;
        let owned = owned_cmdline(&dirs);
        let table = FakeProcessTable::new()
            .with_process(4001, &owned)
            .with_process(4002, "/usr/bin/sleep 60")
            .with_process(4003, "/usr/local/bin/cloudflared tunnel --url http://x");
        let files = [
            write_pidfile(&dirs, "app", "4001\n"),
            write_pidfile(&dirs, "@named", "4002\n"),
            write_pidfile(&dirs, "other", "4003"),
            write_pidfile(&dirs, "garbage", "not a pid"),
            write_pidfile(&dirs, "gone", "4999"),
        ];
        let log = pid_dir(&dirs).join("app.log");
        std::fs::write(&log, "keep").unwrap();

        let reaped = reap_with(&table, &dirs, Duration::from_millis(200));

        assert_eq!(reaped, vec![4001]);
        assert_eq!(
            table.signals(),
            vec![RecordedSignal {
                pid: 4001,
                force: false
            }]
        );
        assert!(table.is_alive(4002));
        assert!(table.is_alive(4003));
        for file in files {
            assert!(!file.exists(), "{}", file.display());
        }
        assert!(log.exists());
    }

    #[test]
    fn reap_escalates_to_a_forced_kill_when_the_orphan_survives() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = state_in(tmp.path()).dirs;
        let table = FakeProcessTable::new().with_stubborn_process(4001, &owned_cmdline(&dirs));
        write_pidfile(&dirs, "@named", "4001");

        reap_with(&table, &dirs, Duration::from_millis(150));

        assert_eq!(
            table.signals(),
            vec![
                RecordedSignal {
                    pid: 4001,
                    force: false
                },
                RecordedSignal {
                    pid: 4001,
                    force: true
                },
            ]
        );
        assert!(!table.is_alive(4001));
    }

    #[test]
    fn reap_does_not_force_kill_a_reused_pid() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = state_in(tmp.path()).dirs;
        let table = FakeProcessTable::new().with_stubborn_process(4001, &owned_cmdline(&dirs));
        write_pidfile(&dirs, "@named", "4001");
        let reused = FakeProcessTable::new().with_stubborn_process(4001, "/usr/bin/sleep 60");
        let swapped = SwapAfterTerm {
            before: table,
            after: reused,
            termed: std::sync::atomic::AtomicBool::new(false),
        };

        reap_with(&swapped, &dirs, Duration::from_millis(150));

        assert_eq!(
            swapped.before.signals(),
            vec![RecordedSignal {
                pid: 4001,
                force: false
            }]
        );
        assert!(swapped.after.signals().is_empty());
    }

    /// Reads from `before` until the first SIGTERM, then from `after`, standing
    /// in for an orphan that exits and has its PID reused during the grace.
    struct SwapAfterTerm {
        before: FakeProcessTable,
        after: FakeProcessTable,
        termed: std::sync::atomic::AtomicBool,
    }

    impl SwapAfterTerm {
        fn current(&self) -> &FakeProcessTable {
            if self.termed.load(std::sync::atomic::Ordering::SeqCst) {
                &self.after
            } else {
                &self.before
            }
        }
    }

    impl ProcessTable for SwapAfterTerm {
        fn command_line(&self, pid: u32) -> Option<String> {
            self.current().command_line(pid)
        }

        fn signal_group(&self, pid: u32, force: bool) {
            self.current().signal_group(pid, force);
            self.termed.store(true, std::sync::atomic::Ordering::SeqCst);
        }

        fn is_alive(&self, pid: u32) -> bool {
            self.current().is_alive(pid)
        }
    }

    #[test]
    fn reap_never_signals_reserved_pids() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = state_in(tmp.path()).dirs;
        let owned = owned_cmdline(&dirs);
        let table = FakeProcessTable::new()
            .with_process(0, &owned)
            .with_process(1, &owned);
        write_pidfile(&dirs, "a", "0");
        write_pidfile(&dirs, "b", "1");

        let reaped = reap_with(&table, &dirs, Duration::from_millis(50));

        assert!(reaped.is_empty());
        assert!(table.signals().is_empty());
    }

    #[test]
    fn reap_without_a_pid_dir_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = state_in(tmp.path()).dirs;
        let table = FakeProcessTable::new();

        assert!(reap_with(&table, &dirs, Duration::from_millis(50)).is_empty());
    }
}
