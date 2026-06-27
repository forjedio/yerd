//! Effective-UID check.
//!
//! Linux reads `/proc/self/status`; macOS shells out to `/usr/bin/id`
//! by absolute path so a poisoned `PATH` from the elevation mechanism
//! cannot redirect the lookup. If `/proc` is missing (chroot, minimal
//! container), Linux conservatively reports `false` - better to fail
//! with `NotPrivileged` than to assume we're root.
//!
//! Neither path uses `unsafe` FFI to `geteuid`, which is forbidden by
//! the workspace `unsafe_code = "forbid"` lint.

#[cfg(target_os = "linux")]
use std::fs;

/// True iff the helper's effective UID is 0.
#[must_use]
pub fn is_privileged() -> bool {
    effective_uid() == Some(0)
}

#[cfg(target_os = "linux")]
fn effective_uid() -> Option<u32> {
    let text = fs::read_to_string("/proc/self/status").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            // Format: "Uid:\treal\teffective\tsaved\tfsuid"
            let mut fields = rest.split_whitespace();
            let _real = fields.next()?;
            let effective = fields.next()?;
            return effective.parse().ok();
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn effective_uid() -> Option<u32> {
    let out = std::process::Command::new("/usr/bin/id")
        .arg("-u")
        .env_clear()
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    s.trim().parse().ok()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn effective_uid() -> Option<u32> {
    None
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn is_privileged_returns_a_bool() {
        let _ = is_privileged();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn effective_uid_parses_status_format() {
        assert!(effective_uid().is_some());
    }
}
