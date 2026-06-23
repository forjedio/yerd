//! Yerd CLI — a thin `yerd-ipc` client of the `yerdd` daemon.
//!
//! Binary-only crates don't expose a Rust API to integration tests under
//! `tests/`. This lib publishes the CLI's modules so `tests/cli_e2e.rs` can
//! drive the pure mapping (`map`) and the transport (`transport`) against a
//! daemon booted on a tempdir. All behaviour lives in the modules; `main.rs`
//! is a thin wrapper around [`run`].

#![forbid(unsafe_code)]

pub mod cli;
#[cfg(unix)]
pub mod composer_shim;
#[cfg(unix)]
pub mod cover_shim;
pub mod elevate;
pub mod error;
#[cfg(unix)]
pub mod laravel_shim;
pub mod map;
pub mod path_cmd;
#[cfg(unix)]
pub mod shim;
pub mod transport;
pub mod uninstall;

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
        // `path` edits the user's shell rc file(s); fully local, no IPC.
        Command::Path { action } => return path_cmd::run(*action),
        // Bare `yerd uninstall` (no target) tears yerd down locally: it stops
        // the daemon and deletes files, so it can't go over IPC. `uninstall
        // php/tool` keeps its daemon-mediated path below.
        Command::Uninstall { target: None, yes } => return uninstall::run(*yes),
        // Stream the install output (Composer's, for the Laravel installer) line by
        // line. JSON mode keeps the plain blocking path for clean machine output.
        Command::Install {
            target: crate::cli::InstallTarget::Tool { id },
        } if !cli.json => return stream_install_tool(id, cli.json).await,
        _ => {}
    }

    let req = match map::to_request(&cli.command)
        .map(canonicalize_unpark)
        .and_then(canonicalize_db_paths)
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("yerd: {e}");
            // `to_request` / path resolution only fail with client-side usage errors.
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
            // After installing a dev tool, wire Yerd's bin dir onto PATH so the
            // tool's commands resolve in a new shell (idempotent; quiet if
            // already configured). The Doctor `BinDirNotOnPath` warning backstops
            // the GUI / any case this can't run.
            if r.code == 0
                && matches!(
                    cli.command,
                    Command::Install {
                        target: crate::cli::InstallTarget::Tool { .. }
                    }
                )
            {
                path_cmd::ensure_installed_after_tool(cli.json);
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

/// Install a dev tool as a streamed job, printing its output line by line until
/// the job reaches a terminal state. Mirrors the GUI's streamed install.
async fn stream_install_tool(id: &str, json: bool) -> ExitCode {
    use std::time::Duration;
    use yerd_ipc::{JobState, Request, Response};

    let job_id = match transport::exchange(&Request::InstallToolStreamed {
        tool: id.to_owned(),
    })
    .await
    {
        Ok(Response::JobStarted { job_id }) => job_id,
        Ok(Response::Error { message, .. }) => {
            eprintln!("yerd: {message}");
            return ExitCode::from(1);
        }
        Ok(_) => {
            eprintln!("yerd: unexpected response starting install");
            return ExitCode::from(74);
        }
        Err(e @ ClientError::DaemonUnreachable(_)) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(69);
        }
        Err(e) => {
            eprintln!("yerd: {e}");
            return ExitCode::from(74);
        }
    };

    let mut cursor = 0u64;
    loop {
        match transport::exchange(&Request::JobStatus {
            job_id: job_id.clone(),
            cursor,
        })
        .await
        {
            Ok(Response::JobProgress {
                state,
                log,
                next_cursor,
                error,
                ..
            }) => {
                for line in &log {
                    println!("{line}");
                }
                cursor = next_cursor;
                match state {
                    JobState::Running => tokio::time::sleep(Duration::from_millis(400)).await,
                    JobState::Succeeded => {
                        // Wire Yerd's bin dir onto PATH so the tool's commands
                        // resolve in a new shell (same as the blocking path).
                        path_cmd::ensure_installed_after_tool(json);
                        return ExitCode::SUCCESS;
                    }
                    JobState::Failed => {
                        if let Some(e) = error {
                            eprintln!("yerd: {e}");
                        }
                        return ExitCode::from(1);
                    }
                    JobState::Cancelled => {
                        eprintln!("yerd: install cancelled");
                        return ExitCode::from(1);
                    }
                }
            }
            Ok(Response::Error { message, .. }) => {
                eprintln!("yerd: {message}");
                return ExitCode::from(1);
            }
            Ok(_) => {
                eprintln!("yerd: unexpected response polling install");
                return ExitCode::from(74);
            }
            Err(e @ ClientError::DaemonUnreachable(_)) => {
                eprintln!("yerd: {e}");
                return ExitCode::from(69);
            }
            Err(e) => {
                eprintln!("yerd: {e}");
                return ExitCode::from(74);
            }
        }
    }
}

/// Best-effort: rewrite an `Unpark` request's path to its canonical form so a
/// relative or symlinked path the user typed matches the canonical string the
/// daemon stored when the directory was parked. The daemon matches `unpark`
/// *exactly* (it deliberately does not canonicalise — so a directory deleted
/// from disk is still removable by its exact stored path); doing it here, at the
/// I/O boundary, keeps `map::to_request` pure. A path that can't be canonicalised
/// (e.g. already deleted) is left exactly as typed.
fn canonicalize_unpark(req: yerd_ipc::Request) -> yerd_ipc::Request {
    if let yerd_ipc::Request::Unpark { path } = &req {
        if let Ok(canon) = std::fs::canonicalize(path) {
            return yerd_ipc::Request::Unpark {
                path: canon.to_string_lossy().into_owned(),
            };
        }
    }
    req
}

/// Absolutise the file path of a `BackupDatabase`/`RestoreDatabase` request against
/// the user's current directory before it reaches the daemon — the daemon's own cwd
/// differs from the user's shell, so a relative path would otherwise resolve in the
/// wrong place. Done here, at the I/O boundary, to keep `map::to_request` pure.
///
/// Restore requires the source file to exist (canonicalise, fail loudly if missing);
/// backup's destination does not exist yet, so it is merely made absolute.
fn canonicalize_db_paths(req: yerd_ipc::Request) -> Result<yerd_ipc::Request, ClientError> {
    use yerd_ipc::Request;
    match req {
        Request::RestoreDatabase {
            service,
            name,
            path,
        } => {
            let path = std::fs::canonicalize(&path).map_err(|e| {
                ClientError::Usage(format!("cannot read backup file {}: {e}", path.display()))
            })?;
            Ok(Request::RestoreDatabase {
                service,
                name,
                path,
            })
        }
        Request::BackupDatabase {
            service,
            name,
            path,
        } => Ok(Request::BackupDatabase {
            service,
            name,
            path: absolutise(&path)?,
        }),
        other => Ok(other),
    }
}

/// Make a (possibly relative) path absolute by joining it onto the current directory.
/// Does not require the path to exist (used for a backup destination).
fn absolutise(path: &std::path::Path) -> Result<std::path::PathBuf, ClientError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir()
        .map_err(|e| ClientError::Usage(format!("cannot resolve current directory: {e}")))?;
    Ok(cwd.join(path))
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
    println!("  (also provides `php<ver>` and `phpcover`/`php<ver>cover` for pcov coverage)");
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
