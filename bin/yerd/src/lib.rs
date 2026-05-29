//! Yerd CLI — a thin `yerd-ipc` client of the `yerdd` daemon.
//!
//! Binary-only crates don't expose a Rust API to integration tests under
//! `tests/`. This lib publishes the CLI's modules so `tests/cli_e2e.rs` can
//! drive the pure mapping (`map`) and the transport (`transport`) against a
//! daemon booted on a tempdir. All behaviour lives in the modules; `main.rs`
//! is a thin wrapper around [`run`].

#![forbid(unsafe_code)]

pub mod cli;
pub mod error;
pub mod map;
pub mod transport;

use std::process::ExitCode;

pub use error::ClientError;

use cli::Cli;

/// Map the parsed command to a request, exchange it with the daemon, and
/// render the response. Returns the process exit code:
/// `0` success, `1` daemon error response, `2` usage error, `69` daemon
/// unreachable, `74` other transport/IO failure.
pub async fn run(cli: Cli) -> ExitCode {
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
            ExitCode::from(r.code)
        }
        Err(e @ ClientError::DaemonUnreachable(_)) => {
            eprintln!("yerd: {e}");
            ExitCode::from(69)
        }
        Err(e) => {
            eprintln!("yerd: {e}");
            ExitCode::from(74)
        }
    }
}
