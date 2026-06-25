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
/// The two CLI verbs we drive. Validated (like `TARGETS`) before reaching
/// `spawn_elevated`, where `verb` is interpolated into the macOS AppleScript —
/// both come from fixed allowlists, keeping that string injection-safe.
const VERBS: [&str; 2] = ["elevate", "unelevate"];

/// Validate the verb + target and run the elevated command, returning when it
/// exits. `verb` is `"elevate"` or `"unelevate"`; an **empty** `target` means
/// "all" — the CLI applies every step (`yerd elevate` with no subcommand).
pub async fn run(verb: &str, target: &str) -> Result<(), GuiError> {
    if !VERBS.contains(&verb) {
        return Err(GuiError::internal(format!("unknown verb: {verb}")));
    }
    if !target.is_empty() && !TARGETS.contains(&target) {
        return Err(GuiError::internal(format!(
            "unknown elevate target: {target}"
        )));
    }
    let yerd = trusted_yerd()?;
    let verb = verb.to_owned();
    let target = target.to_owned();
    // Spawn the blocking, prompt-driven process off the async runtime.
    let result = tokio::task::spawn_blocking(move || spawn_elevated(&yerd, &verb, &target))
        .await
        .map_err(|e| GuiError::internal(format!("join error: {e}")))?;
    result
}

/// Apply **multiple** targets under a single OS elevation prompt (macOS), so
/// "Fix all" doesn't ask for the password once per target. `targets` must be
/// non-empty and each from [`TARGETS`].
pub async fn run_many(verb: &str, targets: &[&str]) -> Result<(), GuiError> {
    if !VERBS.contains(&verb) {
        return Err(GuiError::internal(format!("unknown verb: {verb}")));
    }
    if targets.is_empty() {
        return Err(GuiError::internal("no elevate targets given"));
    }
    for t in targets {
        if !TARGETS.contains(t) {
            return Err(GuiError::internal(format!("unknown elevate target: {t}")));
        }
    }
    let yerd = trusted_yerd()?;
    let verb = verb.to_owned();
    let targets: Vec<String> = targets.iter().map(|s| (*s).to_owned()).collect();
    tokio::task::spawn_blocking(move || spawn_elevated_many(&yerd, &verb, &targets))
        .await
        .map_err(|e| GuiError::internal(format!("join error: {e}")))?
}

/// The `yerd` binary that sits beside this app's executable.
fn trusted_yerd() -> Result<PathBuf, GuiError> {
    // Refuse to run a privileged helper from a translocated bundle: `current_exe`
    // would be an ephemeral `/AppTranslocation/…` path that vanishes on unmount,
    // and root would be spawned from a non-stable location.
    #[cfg(target_os = "macos")]
    if crate::autostart::is_translocated() {
        return Err(GuiError::internal(
            "Move Yerd to your Applications folder, then try again \
             (it's running from a temporary location).",
        ));
    }
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
fn spawn_elevated(yerd: &std::path::Path, verb: &str, target: &str) -> Result<(), GuiError> {
    // `pkexec /usr/bin/env SUDO_UID=<uid> <yerd> <verb> [<target>]`
    // (an empty target means "all", so the subcommand arg is omitted).
    let uid = current_uid();
    let mut cmd = std::process::Command::new("pkexec");
    cmd.arg("/usr/bin/env")
        .arg(format!("SUDO_UID={uid}"))
        .arg(yerd)
        .arg(verb);
    if !target.is_empty() {
        cmd.arg(target);
    }
    let status = cmd
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
            "yerd {verb} exited with status {c}"
        ))),
        None => Err(GuiError::internal(format!(
            "yerd {verb} was terminated by a signal"
        ))),
    }
}

#[cfg(target_os = "macos")]
fn spawn_elevated(yerd: &std::path::Path, verb: &str, target: &str) -> Result<(), GuiError> {
    let uid = current_uid();
    let path = applescript_escape(&yerd.to_string_lossy());
    // Single shell invocation: `env SUDO_UID=N <yerd> <verb> [<target>]`.
    let script = format!(
        "do shell script {} with administrator privileges",
        shell_chunk(uid, &path, verb, target),
    );
    run_osascript(&script, verb)
}

/// Apply **several** targets in ONE `with administrator privileges` prompt by
/// chaining them with `&&` inside a single `do shell script`, so "Fix all" asks
/// for the password once instead of once per target. `targets` is non-empty and
/// each entry is from the validated allowlist (see [`run_many`]).
#[cfg(target_os = "macos")]
fn spawn_elevated_many(
    yerd: &std::path::Path,
    verb: &str,
    targets: &[String],
) -> Result<(), GuiError> {
    let uid = current_uid();
    let path = applescript_escape(&yerd.to_string_lossy());
    // Each chunk is an AppleScript string expression for one invocation; join the
    // chunks with a literal ` && ` so the shell runs them in sequence under one
    // elevation.
    let joined = targets
        .iter()
        .map(|t| shell_chunk(uid, &path, verb, t))
        .collect::<Vec<_>>()
        .join(" & \" && \" & ");
    let script = format!("do shell script {joined} with administrator privileges");
    run_osascript(&script, verb)
}

/// One AppleScript string expression that yields the shell command
/// `env SUDO_UID=N <yerd> <verb> [<target>]`. The `yerd` path goes through
/// `quoted form of` (shell-safe regardless of spaces/specials); `verb`/`target`
/// are from fixed allowlists validated by callers, so they're injection-safe.
/// `SUDO_UID` MUST be embedded — `osascript … with administrator privileges` runs
/// as root with a clean env and does NOT set `SUDO_UID` (a `sudo`-ism), yet
/// `yerd elevate` relies on it for socket lookup and the CA owner-check.
#[cfg(target_os = "macos")]
fn shell_chunk(uid: u32, escaped_path: &str, verb: &str, target: &str) -> String {
    // Empty target → bare verb (the CLI then applies all steps).
    let tail = if target.is_empty() {
        format!(" {verb}")
    } else {
        format!(" {verb} {target}")
    };
    format!("\"env SUDO_UID={uid} \" & quoted form of \"{escaped_path}\" & \"{tail}\"")
}

/// Pipe an AppleScript to `osascript` (on stdin, not a fragile `-e` one-liner) and
/// translate the outcome into a [`GuiError`].
#[cfg(target_os = "macos")]
fn run_osascript(script: &str, verb: &str) -> Result<(), GuiError> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

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
        "yerd {verb} failed: {}",
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
fn spawn_elevated(_yerd: &std::path::Path, _verb: &str, _target: &str) -> Result<(), GuiError> {
    Err(GuiError::internal(
        "in-app elevation is not supported on this platform; run `yerd elevate` in a terminal",
    ))
}

/// Non-macOS batch: apply each target in turn. The GUI only uses the batched path
/// on macOS (Linux "Fix all" uses the single all-in-one `yerd elevate` via
/// [`run`]), so this is a correctness fallback rather than a one-prompt path.
#[cfg(not(target_os = "macos"))]
fn spawn_elevated_many(
    yerd: &std::path::Path,
    verb: &str,
    targets: &[String],
) -> Result<(), GuiError> {
    for t in targets {
        spawn_elevated(yerd, verb, t)?;
    }
    Ok(())
}

/// The effective uid of the (unprivileged) GUI process.
#[cfg(unix)]
pub(crate) fn current_uid() -> u32 {
    // SAFETY: `geteuid` is an always-succeeding syscall with no preconditions
    // and no memory effects; it cannot fail or invoke UB.
    unsafe { libc::geteuid() }
}
