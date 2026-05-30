//! `yerdd` entry point. Parses CLI args, installs tracing, runs the
//! tokio runtime, and translates `DaemonError` into a sysexits-style
//! exit code.

use std::process::ExitCode;

use clap::Parser;

use yerdd::args::{Cli, Command, ServeArgs};
use yerdd::{error, run, tracing_init, Outcome};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Command::Serve(args) = cli
        .command
        .unwrap_or_else(|| Command::Serve(ServeArgs::default()));
    tracing_init::init(args.verbose);

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("yerdd: cannot build tokio runtime: {e}");
            return ExitCode::from(70);
        }
    };

    let outcome = runtime.block_on(run(args));
    match outcome {
        Ok(Outcome::Exit) => ExitCode::SUCCESS,
        Ok(Outcome::Restart) => {
            // Drop the runtime first so worker threads are joined and no
            // residual fd survives into the new image, then re-exec in place.
            drop(runtime);
            tracing::info!("restarting daemon (re-exec)");
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
