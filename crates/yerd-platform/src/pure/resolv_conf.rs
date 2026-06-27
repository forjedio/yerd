//! Decide whether `systemd-resolved` is in charge given the text of
//! `/etc/resolv.conf` and a flag for the existence of
//! `/run/systemd/resolve`.
//!
//! The detection is conservative: positive only if the resolv.conf marker
//! line is present or the runtime directory exists. False is a hard "go to
//! the `/etc/resolv.conf` backend".

/// Returns `true` if `systemd-resolved` appears to be in charge.
///
/// Inputs:
/// - `resolv_conf_text` - verbatim content of `/etc/resolv.conf`.
/// - `run_systemd_resolve_exists` - `std::fs::metadata("/run/systemd/resolve").is_ok()`.
///
/// Either piece of evidence is sufficient. A symlink to
/// `stub-resolv.conf` produces the marker comment Linux distros use, but
/// some systems run `systemd-resolved` without the stub link, so the
/// runtime-directory check covers them.
#[must_use]
pub fn detect_systemd_resolved(resolv_conf_text: &str, run_systemd_resolve_exists: bool) -> bool {
    if run_systemd_resolve_exists {
        return true;
    }
    resolv_conf_text
        .lines()
        .take(8)
        .any(|line| line.contains("systemd-resolved"))
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
    fn runtime_dir_present_short_circuits_true() {
        assert!(detect_systemd_resolved("", true));
    }

    #[test]
    fn marker_in_first_lines_detects() {
        let text = "# This file is managed by man:systemd-resolved(8). Do not edit.\nnameserver 127.0.0.53\n";
        assert!(detect_systemd_resolved(text, false));
    }

    #[test]
    fn marker_buried_past_8_lines_is_not_detected() {
        let mut text = String::new();
        for _ in 0..8 {
            text.push_str("# filler\n");
        }
        text.push_str("# this file is managed by systemd-resolved\n");
        assert!(!detect_systemd_resolved(&text, false));
    }

    #[test]
    fn empty_text_and_no_runtime_dir_is_false() {
        assert!(!detect_systemd_resolved("", false));
    }

    #[test]
    fn plain_resolv_conf_without_marker_is_false() {
        let text = "nameserver 8.8.8.8\nnameserver 1.1.1.1\n";
        assert!(!detect_systemd_resolved(text, false));
    }
}
