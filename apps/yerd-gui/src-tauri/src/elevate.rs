//! OS-elevated invocation of the existing `yerd elevate` CLI.
//!
//! Invariants (see the plan's elevation section, grounded in
//! `bin/yerd/src/elevate.rs`):
//!   1. Elevate the CLI, not the GUI — the GUI process never becomes root.
//!   2. Resolve the trusted `yerd` path as a sibling of our own `current_exe`,
//!      never from `PATH` or the daemon (anti-forgery, like the CLI does).
//!   3. Thread the real uid through `env SUDO_UID=<uid>` because `pkexec`
//!      clears the environment and sets only `PKEXEC_UID`; `yerd elevate`
//!      locates the user's socket and owner-checks the CA from `SUDO_UID`.
//!
//! Linux is fully wired (`pkexec`). Other platforms return an explanatory error
//! — the frontend already gates the in-app "Fix" to Linux (macOS needs a CLI
//! socket-path fix first), so this path is not reached there.

use std::path::PathBuf;

use crate::error::GuiError;

const TARGETS: [&str; 3] = ["trust", "resolver", "ports"];

/// Validate the target and run the elevated command, returning when it exits.
pub async fn run(target: &str) -> Result<(), GuiError> {
    if !TARGETS.contains(&target) {
        return Err(GuiError::internal(format!(
            "unknown elevate target: {target}"
        )));
    }
    let yerd = trusted_yerd()?;
    let target = target.to_owned();
    // Spawn the blocking, prompt-driven process off the async runtime.
    let result = tokio::task::spawn_blocking(move || spawn_elevated(&yerd, &target))
        .await
        .map_err(|e| GuiError::internal(format!("join error: {e}")))?;
    result
}

/// The `yerd` binary that sits beside this app's executable.
fn trusted_yerd() -> Result<PathBuf, GuiError> {
    let exe = std::env::current_exe()
        .map_err(|e| GuiError::internal(format!("cannot resolve current exe: {e}")))?;
    let dir = exe
        .parent()
        .ok_or_else(|| GuiError::internal("app executable has no parent directory"))?;
    let cand = dir.join("yerd");
    if cand.is_file() {
        Ok(cand)
    } else {
        Err(GuiError::internal(format!(
            "the yerd CLI was not found beside the app at {}",
            cand.display()
        )))
    }
}

#[cfg(target_os = "linux")]
fn spawn_elevated(yerd: &std::path::Path, target: &str) -> Result<(), GuiError> {
    // `pkexec /usr/bin/env SUDO_UID=<uid> <yerd> elevate <target>`
    let uid = current_uid();
    let status = std::process::Command::new("pkexec")
        .arg("/usr/bin/env")
        .arg(format!("SUDO_UID={uid}"))
        .arg(yerd)
        .arg("elevate")
        .arg(target)
        .status()
        .map_err(|e| GuiError::internal(format!("failed to launch pkexec: {e}")))?;

    if status.success() {
        return Ok(());
    }
    // pkexec: 126 = user dismissed/not authorized, 127 = auth could not start.
    match status.code() {
        Some(126) => Err(GuiError::internal("authorization was dismissed or denied")),
        Some(127) => Err(GuiError::internal("authentication could not be started")),
        Some(c) => Err(GuiError::internal(format!(
            "yerd elevate exited with status {c}"
        ))),
        None => Err(GuiError::internal(
            "yerd elevate was terminated by a signal",
        )),
    }
}

#[cfg(not(target_os = "linux"))]
fn spawn_elevated(_yerd: &std::path::Path, _target: &str) -> Result<(), GuiError> {
    Err(GuiError::internal(
        "in-app elevation is currently Linux-only; run `yerd elevate` in a terminal",
    ))
}

/// The effective uid of the (unprivileged) GUI process.
#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: `geteuid` is an always-succeeding syscall with no preconditions
    // and no memory effects; it cannot fail or invoke UB.
    unsafe { libc::geteuid() }
}
