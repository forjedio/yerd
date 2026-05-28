//! Privileged one-shot binary for Yerd.
//!
//! The daemon (`yerdd`) runs unprivileged. Operations that require root
//! are sent here as typed `HelperInvocation`s over a frozen argv
//! contract. This binary validates everything (defence in depth),
//! performs exactly one operation, and exits with a `sysexits.h` code
//! the daemon can interpret.

#![forbid(unsafe_code)]

mod cli;
mod error;
mod exec;
mod ops;
mod privilege;
mod validate;

use std::process::ExitCode;

fn main() -> ExitCode {
    // Windows is not supported in Phase 1; stub exits 78 (EX_CONFIG).
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        eprintln!("yerd-helper: not supported on this OS");
        return ExitCode::from(78);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        run()
    }
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

    // Defang any relative-path argv before doing any per-op work.
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
