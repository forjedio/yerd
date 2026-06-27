//! Pure parsers for Linux `/proc` metric files.
//!
//! Both functions are I/O-free: the OS layer reads the file and hands the
//! contents here. Kept lenient - a malformed line yields `None` rather than a
//! panic, because metrics are best-effort.

/// Parse the resident set size (in bytes) from the contents of
/// `/proc/<pid>/status`.
///
/// Looks for the `VmRSS:` line, whose value is in kibibytes
/// (`VmRSS:\t  12345 kB`), and multiplies by 1024. Using `VmRSS` (already a
/// byte-ish unit) avoids needing the page size (`_SC_PAGESIZE`) that the
/// `statm` file would require, keeping this parser dependency- and `unsafe`-free.
///
/// Returns `None` if there is no `VmRSS:` line or its number does not parse.
#[must_use]
pub fn parse_vmrss_bytes(status_contents: &str) -> Option<u64> {
    for line in status_contents.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kib.saturating_mul(1024));
        }
    }
    None
}

/// Parse the first three load-average figures from the contents of
/// `/proc/loadavg` (e.g. `0.52 0.48 0.44 1/523 12345`).
///
/// Returns `None` unless all three parse as floats.
#[must_use]
pub fn parse_loadavg(loadavg_contents: &str) -> Option<[f64; 3]> {
    let mut it = loadavg_contents.split_whitespace();
    let one: f64 = it.next()?.parse().ok()?;
    let five: f64 = it.next()?.parse().ok()?;
    let fifteen: f64 = it.next()?.parse().ok()?;
    Some([one, five, fifteen])
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]
mod tests {
    use super::*;

    #[test]
    fn vmrss_parses_kib_to_bytes() {
        let status = "Name:\tphp-fpm\nVmPeak:\t  900000 kB\nVmRSS:\t   12345 kB\nThreads:\t1\n";
        assert_eq!(parse_vmrss_bytes(status), Some(12345 * 1024));
    }

    #[test]
    fn vmrss_missing_line_is_none() {
        assert_eq!(parse_vmrss_bytes("Name:\tphp-fpm\nThreads:\t1\n"), None);
    }

    #[test]
    fn vmrss_garbage_value_is_none() {
        assert_eq!(parse_vmrss_bytes("VmRSS:\tnope kB\n"), None);
    }

    #[test]
    fn vmrss_zero_is_zero_not_none() {
        assert_eq!(parse_vmrss_bytes("VmRSS:\t0 kB\n"), Some(0));
    }

    #[test]
    fn loadavg_parses_three_floats() {
        let parsed = parse_loadavg("0.52 0.48 0.44 1/523 12345").unwrap();
        assert_eq!(parsed, [0.52, 0.48, 0.44]);
    }

    #[test]
    fn loadavg_trailing_newline_ok() {
        let parsed = parse_loadavg("1.00 2.00 3.00 2/100 9\n").unwrap();
        assert_eq!(parsed, [1.00, 2.00, 3.00]);
    }

    #[test]
    fn loadavg_too_few_fields_is_none() {
        assert_eq!(parse_loadavg("0.52 0.48"), None);
    }

    #[test]
    fn loadavg_garbage_is_none() {
        assert_eq!(parse_loadavg("x y z 1/2 3"), None);
    }
}
