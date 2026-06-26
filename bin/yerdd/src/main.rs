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
    let Command::Serve(args) = cli
        .command
        .unwrap_or_else(|| Command::Serve(ServeArgs::default()));

    // Resolve the cache dir for the rolling daemon log *before* installing
    // tracing, so an early failure (even the runtime-build error below) is
    // captured. A resolve/create failure degrades to stderr-only logging.
    let log_dir = resolve_log_dir();
    // The guard MUST outlive every log call — it's declared before the runtime
    // so locals drop in reverse order: `runtime` first (joining workers, which
    // flushes their logs into the appender channel), this guard last. Named (not
    // `_`-prefixed) because the Restart arm drops it explicitly before `exec`.
    let log_guard = tracing_init::init(args.verbose, log_dir.as_deref());

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            // `tracing` is already installed, so this lands in the rolling log
            // too (not just stderr) — otherwise this failure would be invisible
            // under launchd/systemd.
            tracing::error!(error = %e, "yerdd: cannot build tokio runtime");
            return ExitCode::from(70);
        }
    };

    let outcome = runtime.block_on(run(args));
    match outcome {
        Ok(Outcome::Exit) => ExitCode::SUCCESS,
        Ok(Outcome::Restart) => {
            // Drop the runtime first so worker threads are joined and no
            // residual fd survives into the new image.
            drop(runtime);
            tracing::info!("restarting daemon (re-exec)");
            // `exec` runs no destructors, so flush the log worker explicitly
            // here — and only *after* the line above, or it's lost from the file.
            drop(log_guard);
            match restart_in_place() {
                Ok(()) => unreachable!("exec replaces the process on success"),
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

/// Resolve `{cache}/` for the daemon log and ensure it exists. Returns `None`
/// (→ stderr-only logging) if dirs can't be resolved or the directory can't be
/// created — logging must never be a hard failure for the daemon.
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
/// `exec` failed. Unix-only — the daemon refuses `RestartDaemon` elsewhere, so
/// `Outcome::Restart` is unreachable on non-Unix.
#[cfg(unix)]
fn restart_in_place() -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()?;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // `exec()` only returns (an error) on failure.
    Err(std::process::Command::new(exe).args(args).exec())
}

#[cfg(not(unix))]
fn restart_in_place() -> std::io::Result<()> {
    Err(std::io::Error::other(
        "daemon restart is not supported on this platform",
    ))
}
