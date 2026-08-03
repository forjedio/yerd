//! `yerdd` entry point. Parses CLI args, installs tracing, runs the
//! tokio runtime, and translates `DaemonError` into a sysexits-style
//! exit code.

use std::process::ExitCode;

use clap::Parser;

use yerd_platform::{ActivePaths, Paths};
use yerdd::args::{Cli, Command, ServeArgs};
use yerdd::{error, run, tracing_init, Outcome};

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.pkg_format {
        println!("{}", pkg_format_str());
        return ExitCode::SUCCESS;
    }
    let Command::Serve(args) = cli
        .command
        .unwrap_or_else(|| Command::Serve(ServeArgs::default()));

    #[cfg(windows)]
    if args.detach {
        return relaunch_detached(&args);
    }

    let log_dir = resolve_log_dir();
    let log_guard = tracing_init::init(args.verbose, log_dir.as_deref());

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "yerdd: cannot build tokio runtime");
            return ExitCode::from(70);
        }
    };

    let outcome = runtime.block_on(run(args));
    match outcome {
        Ok(Outcome::Exit) => ExitCode::SUCCESS,
        Ok(Outcome::Restart) => {
            drop(runtime);
            tracing::info!("restarting daemon (re-exec)");
            drop(log_guard);
            match restart_in_place() {
                #[cfg(unix)]
                Ok(()) => unreachable!("exec replaces the process on success"),
                #[cfg(not(unix))]
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("yerdd: re-exec failed: {e}");
                    ExitCode::from(70)
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "yerdd exiting with error");
            ExitCode::from(error::exit_code(&e))
        }
    }
}

/// The build's self-update package format as a stable lowercase string
/// (`"pacman"` under the `pacman` feature, `"rpm"` under the `rpm` feature, else
/// `"deb"`). Used by the hidden `--pkg-format` diagnostic the release pipeline
/// asserts on.
fn pkg_format_str() -> &'static str {
    match yerd_update::PkgFormat::current() {
        yerd_update::PkgFormat::Pacman => "pacman",
        yerd_update::PkgFormat::Rpm => "rpm",
        yerd_update::PkgFormat::Deb => "deb",
    }
}

/// Resolve `{cache}/` for the daemon log and ensure it exists. Returns `None`
/// (→ stderr-only logging) if dirs can't be resolved or the directory can't be
/// created - logging must never be a hard failure for the daemon.
fn resolve_log_dir() -> Option<std::path::PathBuf> {
    let dirs = match ActivePaths::new().resolve() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("yerdd: cannot resolve cache dir for logging: {e}");
            return None;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&dirs.cache) {
        eprintln!("yerdd: cannot create log dir {}: {e}", dirs.cache.display());
        return None;
    }
    Some(dirs.cache)
}

/// Re-exec this binary in place with the original argv (same PID). On success
/// the process image is replaced and this never returns; an `Err` means the
/// `exec` failed.
#[cfg(unix)]
fn restart_in_place() -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()?;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    Err(std::process::Command::new(exe).args(args).exec())
}

/// Spawn a fresh daemon (detached, no console window) with the original argv and
/// the restart-handoff marker, then return `Ok` so `main` exits. This is the
/// Windows **console-mode** restart: `exec` has no equivalent, so a new process
/// replaces this one. By the time `Outcome::Restart` reaches `main` the outgoing
/// daemon has fully torn down (pipe, instance lock, and Job Objects released - see
/// `run_with_daemon`), so the child just re-runs normal startup and bounded-retries
/// the bind under [`yerdd::RESTART_HANDOFF_ENV`]. A foreground console daemon thus
/// restarts into the background; its logs continue in `{cache}/`.
#[cfg(windows)]
fn restart_in_place() -> std::io::Result<()> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let exe = std::env::current_exe()?;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    std::process::Command::new(exe)
        .args(args)
        .env(yerdd::RESTART_HANDOFF_ENV, "1")
        .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
        .spawn()?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn restart_in_place() -> std::io::Result<()> {
    Err(std::io::Error::other(
        "daemon restart is not supported on this platform",
    ))
}

/// Respawn this daemon hidden (no console window) without `--detach`, then exit
/// so the caller returns immediately. The HKCU `Run` autostart entry launches
/// `yerdd serve --detach`; without this relaunch the console-subsystem daemon
/// would keep a visible console for its whole lifetime. Only the parsed
/// verbosity/config flags are forwarded (via [`yerdd::args::respawn_args`], never
/// raw argv), so `--detach` can't leak and loop. `CREATE_NO_WINDOW` (a hidden
/// console, not `DETACHED_PROCESS`) is chosen so the child still receives
/// logoff/shutdown control events for a graceful drain (see `signals.rs`). A
/// brief console flash at logon is the accepted cosmetic cost. Both flags are
/// safe std `creation_flags`. A spawn failure prints to stderr and exits 70.
#[cfg(windows)]
fn relaunch_detached(args: &ServeArgs) -> ExitCode {
    use std::os::windows::process::CommandExt as _;
    use std::process::Stdio;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("yerdd: cannot locate current executable to detach: {e}");
            return ExitCode::from(70);
        }
    };
    match std::process::Command::new(exe)
        .args(yerdd::args::respawn_args(args))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .spawn()
    {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yerdd: failed to spawn the detached daemon: {e}");
            ExitCode::from(70)
        }
    }
}
