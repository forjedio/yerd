//! OS-elevated invocation of the existing `yerd elevate` CLI.
//!
//! Invariants (see the plan's elevation section, grounded in
//! `bin/yerd/src/elevate.rs`):
//!   1. Elevate the CLI, not the GUI — the GUI process never becomes root.
//!   2. Resolve the trusted `yerd` path as a sibling of our own `current_exe`,
//!      never from `PATH` or the daemon (anti-forgery, like the CLI does).
//!   3. Thread the real uid through `env SUDO_UID=<uid>` because the elevation
//!      tool clears the environment; `yerd elevate` locates the user's socket
//!      and owner-checks the CA from `SUDO_UID`.
//!
//! Linux uses `pkexec`; macOS uses `osascript … with administrator privileges`.
//! Windows returns an explanatory error (the frontend gates the in-app "Fix"
//! to Linux/macOS, so that path is not reached there).

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

#[cfg(target_os = "macos")]
fn spawn_elevated(yerd: &std::path::Path, target: &str) -> Result<(), GuiError> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    // Build the AppleScript on stdin (not a fragile `-e` one-liner). The `yerd`
    // path goes through AppleScript's `quoted form of`, making it shell-safe
    // regardless of spaces/specials; `target` is from the fixed allowlist
    // validated in `run`, so it's injection-safe. `SUDO_UID` MUST be embedded —
    // `osascript … with administrator privileges` runs the command as root with
    // a clean env and does NOT set `SUDO_UID` (that's a `sudo`-ism), yet
    // `yerd elevate` relies on it for socket lookup and the CA owner-check.
    let uid = current_uid();
    let yerd_str = yerd.to_string_lossy();
    let script = format!(
        "do shell script \"env SUDO_UID={uid} \" & quoted form of \"{path}\" & \" elevate {target}\" with administrator privileges",
        path = applescript_escape(&yerd_str),
    );

    let mut child = Command::new("/usr/bin/osascript")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GuiError::internal(format!("failed to launch osascript: {e}")))?;
    child
        .stdin
        .take()
        .ok_or_else(|| GuiError::internal("osascript stdin unavailable"))?
        .write_all(script.as_bytes())
        .map_err(|e| GuiError::internal(format!("failed to write osascript script: {e}")))?;
    let out = child
        .wait_with_output()
        .map_err(|e| GuiError::internal(format!("osascript failed: {e}")))?;

    if out.status.success() {
        return Ok(());
    }
    // osascript surfaces a dismissed auth dialog as error -128 / "User
    // canceled", but it ALSO re-raises the inner command's non-zero exit as a
    // numbered error — exit code alone can't tell "dismissed" from "elevate
    // failed", so branch on the stderr text.
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("User canceled") || stderr.contains("-128") {
        return Err(GuiError::internal("authorization was dismissed"));
    }
    Err(GuiError::internal(format!(
        "yerd elevate failed: {}",
        stderr.trim()
    )))
}

/// Escape a string for embedding inside an AppleScript double-quoted string
/// literal. `quoted form of` then handles the shell layer.
#[cfg(target_os = "macos")]
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn spawn_elevated(_yerd: &std::path::Path, _target: &str) -> Result<(), GuiError> {
    Err(GuiError::internal(
        "in-app elevation is not supported on this platform; run `yerd elevate` in a terminal",
    ))
}

/// The effective uid of the (unprivileged) GUI process.
#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: `geteuid` is an always-succeeding syscall with no preconditions
    // and no memory effects; it cannot fail or invoke UB.
    unsafe { libc::geteuid() }
}
