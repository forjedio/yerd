//! Privileged one-shot binary for Yerd.
//!
//! The daemon (`yerdd`) runs unprivileged. Operations that require root
//! (or, on Windows, an elevated token) are sent here as typed
//! `HelperInvocation`s over a frozen argv contract. This binary validates
//! everything (defence in depth), performs exactly one operation, and exits
//! with a `sysexits.h` code the caller can interpret.

#![forbid(unsafe_code)]
// On OSes with no privilege model wired up, `main` returns exit-78 before
// touching the dispatch/validate/ops machinery, so all of it is legitimately
// unreachable. Keep the modules total (they carry per-OS `Unsupported` stubs)
// but silence the unavoidable dead-code warnings the unused internals produce.
#![cfg_attr(
    not(any(target_os = "linux", target_os = "macos", windows)),
    allow(dead_code, unused_imports)
)]

mod cli;
mod error;
mod exec;
mod ops;
mod privilege;
mod validate;

use std::process::ExitCode;

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn main() -> ExitCode {
    eprintln!("yerd-helper: not supported on this OS");
    ExitCode::from(78)
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn main() -> ExitCode {
    run()
}

/// `ShellExecuteEx` already starts the elevated Windows child in `system32`; the
/// Unix `chdir("/")` guards against the elevation mechanism leaving the process
/// in a deleted or attacker-controlled cwd.
///
/// Windows has no stdio back to the caller (again `ShellExecuteEx`), so when the
/// caller supplies a token the outcome goes into an advisory result file
/// instead. Best-effort: it never changes the exit code.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn run() -> ExitCode {
    let parsed = match cli::parse(std::env::args_os()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("yerd-helper: {e}");
            return ExitCode::from(error::exit_code(&e));
        }
    };

    #[cfg(unix)]
    let _ = std::env::set_current_dir("/");

    let result = execute(&parsed);

    #[cfg(windows)]
    if let Some(token) = parsed.result_token.as_deref() {
        ops::write_result_file(token, &result);
    }

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yerd-helper: {e}");
            ExitCode::from(error::exit_code(&e))
        }
    }
}

/// Enforce the privilege check (unless the debug bypass is set), then run the
/// one operation.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn execute(parsed: &cli::ParsedCli) -> Result<(), error::HelperError> {
    if !parsed.skip_priv_check && !privilege::is_privileged() {
        return Err(error::HelperError::NotPrivileged);
    }
    exec::dispatch(parsed.invocation.clone())
}
