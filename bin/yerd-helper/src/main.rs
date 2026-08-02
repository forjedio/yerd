//! Privileged one-shot binary for Yerd.
//!
//! The daemon (`yerdd`) runs unprivileged. Operations that require root
//! are sent here as typed `HelperInvocation`s over a frozen argv
//! contract. This binary validates everything (defence in depth),
//! performs exactly one operation, and exits with a `sysexits.h` code
//! the daemon can interpret.

#![forbid(unsafe_code)]
// On Windows the helper is a compile-only stub: `main` returns exit-78 before
// touching the dispatch/validate/ops machinery, so all of it is legitimately
// unreachable until the Phase 4 Windows privilege model wires it up. Keep the
// modules total (they carry per-OS `Unsupported` stubs) but silence the
// unavoidable dead-code warnings the unused-on-Windows internals produce.
#![cfg_attr(
    not(any(target_os = "linux", target_os = "macos")),
    allow(dead_code, unused_imports)
)]

mod cli;
mod error;
mod exec;
mod ops;
mod privilege;
mod validate;

use std::process::ExitCode;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn main() -> ExitCode {
    eprintln!("yerd-helper: not supported on this OS");
    ExitCode::from(78)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn main() -> ExitCode {
    run()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run() -> ExitCode {
    let parsed = match cli::parse(std::env::args_os()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("yerd-helper: {e}");
            return ExitCode::from(error::exit_code(&e));
        }
    };

    let _ = std::env::set_current_dir("/");

    if !parsed.skip_priv_check && !privilege::is_privileged() {
        let e = error::HelperError::NotPrivileged;
        eprintln!("yerd-helper: {e}");
        return ExitCode::from(error::exit_code(&e));
    }

    if let Err(e) = exec::dispatch(parsed.invocation) {
        eprintln!("yerd-helper: {e}");
        return ExitCode::from(error::exit_code(&e));
    }
    ExitCode::SUCCESS
}
