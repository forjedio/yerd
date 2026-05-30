//! Yerd CLI — a thin `yerd-ipc` client of the `yerdd` daemon.
//!
//! Binary-only crates don't expose a Rust API to integration tests under
//! `tests/`. This lib publishes the CLI's modules so `tests/cli_e2e.rs` can
//! drive the pure mapping (`map`) and the transport (`transport`) against a
//! daemon booted on a tempdir. All behaviour lives in the modules; `main.rs`
//! is a thin wrapper around [`run`].

#![forbid(unsafe_code)]

pub mod cli;
pub mod elevate;
pub mod error;
pub mod map;
pub mod transport;

use std::process::ExitCode;

pub use error::ClientError;

use cli::{Cli, Command};

/// Map the parsed command to a request, exchange it with the daemon, and
/// render the response. Returns the process exit code:
/// `0` success, `1` daemon error response, `2` usage error, `69` daemon
/// unreachable, `74` other transport/IO failure.
pub async fn run(cli: Cli) -> ExitCode {
    // `elevate`/`unelevate` do local privileged orchestration (spawn the
    // helper), not a single IPC round-trip — branch before the IPC path.
    match &cli.command {
        Command::Elevate { target } => return elevate::run_elevate(*target, false).await,
        Command::Unelevate { target } => return elevate::run_elevate(*target, true).await,
        _ => {}
    }

    let req = match map::to_request(&cli.command) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("yerd: {e}");
            // `to_request` only fails with client-side usage errors.
            return ExitCode::from(2);
        }
    };

    match transport::exchange(&req).await {
        Ok(resp) => {
            let r = map::render(&resp, cli.json);
            if !r.stdout.is_empty() {
                println!("{}", r.stdout);
            }
            if !r.stderr.is_empty() {
                eprintln!("{}", r.stderr);
            }
            // After a successful global `yerd use <ver>`, nudge the user about
            // the terminal `php` shim PATH (human output only).
            if !cli.json && r.code == 0 && matches!(cli.command, Command::Use { version: None, .. })
            {
                print_php_path_hint();
            }
            ExitCode::from(r.code)
        }
        Err(e @ ClientError::DaemonUnreachable(_)) => {
            // For `doctor`, a down daemon is itself a FAIL finding: render it as
            // a one-item diagnosis through the normal path so `--json` and the
            // exit code behave like any other doctor run (exits 1). Other
            // commands keep the generic "daemon unreachable" (69) handling.
            if matches!(cli.command, Command::Doctor { .. }) {
                let resp = daemon_down_response();
                let r = map::render(&resp, cli.json);
                if !r.stdout.is_empty() {
                    println!("{}", r.stdout);
                }
                return ExitCode::from(r.code);
            }
            eprintln!("yerd: {e}");
            ExitCode::from(69)
        }
        Err(e) => {
            eprintln!("yerd: {e}");
            ExitCode::from(74)
        }
    }
}

/// A synthetic `daemon_down` FAIL diagnosis, used when `yerd doctor` can't reach
/// the daemon. Routed through `map::render` so it honours `--json` and exits 1.
fn daemon_down_response() -> yerd_ipc::Response {
    yerd_ipc::Response::Diagnoses {
        items: vec![yerd_ipc::Diagnosis {
            code: yerd_ipc::DiagnosisCode::DaemonDown,
            severity: yerd_ipc::Severity::Fail,
            title: "Daemon not running".to_owned(),
            detail: "Could not reach the yerd daemon over its IPC socket.".to_owned(),
            remedy: Some("start the daemon: yerdd".to_owned()),
        }],
    }
}

/// Print where the managed `php` shim lives and warn if another `php` already
/// shadows it on `PATH`. Best-effort: silently does nothing if dirs can't be
/// resolved.
fn print_php_path_hint() {
    use yerd_platform::{ActivePaths, Paths};
    let Ok(dirs) = ActivePaths::new().resolve() else {
        return;
    };
    let bin = dirs.data.join("bin");
    println!(
        "→ ensure {} is on your PATH for the `php` command",
        bin.display()
    );
    // Warn if a different `php` is found earlier on PATH (would shadow the shim).
    if let Some(existing) = first_php_on_path() {
        if existing != bin.join("php") {
            println!(
                "  note: `php` currently resolves to {} — put {} earlier on PATH to override",
                existing.display(),
                bin.display()
            );
        }
    }
}

/// First `php` executable found on `PATH`, if any.
fn first_php_on_path() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("php"))
        .find(|candidate| candidate.is_file())
}
