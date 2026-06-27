//! Pure parser for `ps` resident-set-size output (macOS).
//!
//! macOS has no cheap, `unsafe`-free per-process RSS source in `std`, so the OS
//! layer shells out to `ps -o rss= -p <pid>` and hands the captured stdout here.
//! I/O-free and lenient: malformed output yields `None`, since metrics are
//! best-effort.

/// Parse the resident set size (in bytes) from the stdout of
/// `ps -o rss= -p <pid>`.
///
/// `ps` prints the RSS as a single integer in kibibytes (1024-byte units),
/// surrounded by whitespace (e.g. `"  12345\n"`). This takes the first token,
/// parses it, and multiplies by 1024 to match the byte unit used elsewhere
/// (mirroring [`super::proc_metrics::parse_vmrss_bytes`]).
///
/// Returns `None` if the output has no numeric token (e.g. the pid was gone, so
/// `ps` printed nothing) or the number does not parse.
#[must_use]
pub fn parse_ps_rss_bytes(ps_stdout: &str) -> Option<u64> {
    let kib: u64 = ps_stdout.split_whitespace().next()?.parse().ok()?;
    Some(kib.saturating_mul(1024))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn rss_parses_kib_to_bytes() {
        assert_eq!(parse_ps_rss_bytes("  12345\n"), Some(12345 * 1024));
    }

    #[test]
    fn rss_no_padding_ok() {
        assert_eq!(parse_ps_rss_bytes("4096"), Some(4096 * 1024));
    }

    #[test]
    fn rss_empty_is_none() {
        assert_eq!(parse_ps_rss_bytes(""), None);
        assert_eq!(parse_ps_rss_bytes("   \n"), None);
    }

    #[test]
    fn rss_garbage_is_none() {
        assert_eq!(parse_ps_rss_bytes("nope\n"), None);
    }

    #[test]
    fn rss_zero_is_zero_not_none() {
        assert_eq!(parse_ps_rss_bytes("0\n"), Some(0));
    }
}
