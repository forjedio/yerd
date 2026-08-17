//! Pure parser for `tasklist` working-set output (Windows).
//!
//! Windows has no cheap, `unsafe`-free per-process memory source in `std`, so
//! the OS layer shells out to `tasklist.exe /FI "PID eq <pid>" /FO CSV /NH` and
//! hands the captured stdout here. I/O-free and lenient: malformed output yields
//! `None`, since metrics are best-effort.
//!
//! The figure is the process **working set**, the closest Windows analogue of
//! Unix RSS: both count resident physical pages, so the two are comparable for
//! the "how much memory is this using" reading the GUI shows. `tasklist` reports
//! it in KiB.
//!
//! Un-gated (compiled and table-tested on every OS): pure string parsing.

/// Parse the working set (in bytes) from the stdout of
/// `tasklist /FI "PID eq <pid>" /FO CSV /NH`.
///
/// A CSV row looks like `"yerdd.exe","1234","Console","1","12,345 K"`. Only the
/// first row that starts with a quote is considered, which skips a localized
/// `INFO:` banner (the same locale-dodge [`crate::pure::win_port_owner`] uses).
/// The last quoted field is the memory column; every non-digit is dropped from
/// it, which tolerates any locale's thousands separator and the trailing unit
/// letter. `None` when there is no data row, no digits (a filter that matched
/// nothing prints `INFO:` only, and an unavailable figure prints `N/A`), or the
/// value does not fit a `u64`.
///
/// Fields are read by splitting on the quote character, which yields the field
/// contents at every odd index. Splitting on the comma instead would cut a
/// thousands-separated figure such as `12,345 K` in half.
#[must_use]
pub fn parse_tasklist_mem_bytes(tasklist_csv: &str) -> Option<u64> {
    let row = tasklist_csv
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with('"'))?;
    let fields: Vec<&str> = row.split('"').skip(1).step_by(2).collect();
    let last_field = fields.last()?;
    let digits: String = last_field.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    digits
        .parse::<u64>()
        .ok()
        .map(|kib| kib.saturating_mul(1024))
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

    /// A real English-locale row, captured verbatim.
    const ROW: &str = "\"yerdd.exe\",\"8244\",\"Console\",\"1\",\"12,345 K\"\r\n";

    #[test]
    fn parses_a_real_row_as_kib() {
        assert_eq!(parse_tasklist_mem_bytes(ROW), Some(12_345 * 1024));
    }

    /// A locale using `.` as the thousands separator must parse identically:
    /// only ASCII digits are kept.
    #[test]
    fn tolerates_a_dot_thousands_separator() {
        let row = "\"yerdd.exe\",\"8244\",\"Console\",\"1\",\"12.345 K\"\r\n";
        assert_eq!(parse_tasklist_mem_bytes(row), Some(12_345 * 1024));
    }

    /// A filter that matches no process prints only a banner, which must not be
    /// mistaken for a data row.
    #[test]
    fn info_banner_alone_is_none() {
        let out = "INFO: No tasks are running which match the specified criteria.\r\n";
        assert_eq!(parse_tasklist_mem_bytes(out), None);
    }

    /// A banner followed by a real row still finds the row.
    #[test]
    fn skips_a_banner_before_a_data_row() {
        let out = format!("INFO: something localized\r\n{ROW}");
        assert_eq!(parse_tasklist_mem_bytes(&out), Some(12_345 * 1024));
    }

    #[test]
    fn na_memory_is_none() {
        let row = "\"yerdd.exe\",\"8244\",\"Console\",\"1\",\"N/A\"\r\n";
        assert_eq!(parse_tasklist_mem_bytes(row), None);
    }

    #[test]
    fn empty_input_is_none() {
        assert_eq!(parse_tasklist_mem_bytes(""), None);
        assert_eq!(parse_tasklist_mem_bytes("   \r\n"), None);
    }

    /// A genuinely zero working set parses as zero rather than failing.
    #[test]
    fn zero_kib_is_zero_bytes() {
        let row = "\"yerdd.exe\",\"8244\",\"Console\",\"1\",\"0 K\"\r\n";
        assert_eq!(parse_tasklist_mem_bytes(row), Some(0));
    }
}
