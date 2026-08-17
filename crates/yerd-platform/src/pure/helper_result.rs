//! Pure protocol for the advisory result file the Windows helper writes and the
//! CLI reads after an elevated (`ShellExecuteEx`) run, which yields no stdio.
//!
//! Compiled on every OS so Linux/macOS CI table-tests it too. No I/O: the
//! binaries own the file create/read/delete; this module only validates the
//! token, derives the file name, and renders/parses the one-line body.
//!
//! The exit code stays the authoritative contract (the unchanged `sysexits`
//! mapping); this file carries only the human-readable detail. A hex token
//! (not a path) is passed on the wire precisely because the `runas` argv
//! quoting bug makes backslash/space paths unsafe, and a token cannot name a
//! reparse point.

/// Number of hex characters in a result token (16 random bytes).
pub const TOKEN_LEN: usize = 32;

/// One parsed result-file body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperResult {
    /// The operation succeeded.
    Ok,
    /// The operation failed; carries the helper's error detail (possibly empty).
    Error(String),
}

/// Whether `token` is exactly [`TOKEN_LEN`] lowercase hex characters.
#[must_use]
pub fn valid_token(token: &str) -> bool {
    token.len() == TOKEN_LEN
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The advisory file name for `token`: `helper-result-<token>.txt`.
#[must_use]
pub fn result_file_name(token: &str) -> String {
    format!("helper-result-{token}.txt")
}

/// Render an outcome to the one-line file body (no trailing newline added).
#[must_use]
pub fn render(result: &HelperResult) -> String {
    match result {
        HelperResult::Ok => "ok".to_owned(),
        HelperResult::Error(detail) => format!("error: {detail}"),
    }
}

/// Parse the one-line file body back into a [`HelperResult`]. `None` when the
/// body is neither `ok` nor an `error:`-prefixed line.
#[must_use]
pub fn parse(body: &str) -> Option<HelperResult> {
    let line = body.lines().next().unwrap_or("").trim();
    if line == "ok" {
        return Some(HelperResult::Ok);
    }
    if let Some(rest) = line.strip_prefix("error:") {
        return Some(HelperResult::Error(rest.trim().to_owned()));
    }
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
    fn valid_token_accepts_32_lowercase_hex() {
        assert!(valid_token(&"a1".repeat(16)));
        assert!(valid_token("0123456789abcdef0123456789abcdef"));
    }

    #[test]
    fn valid_token_rejects_bad_shapes() {
        assert!(!valid_token(""));
        assert!(!valid_token(&"a".repeat(31)));
        assert!(!valid_token(&"a".repeat(33)));
        assert!(!valid_token(&"AB".repeat(16)), "uppercase rejected");
        assert!(!valid_token(&"g".repeat(32)), "non-hex rejected");
        assert!(!valid_token("../..///etc//passwd//////////////"));
    }

    #[test]
    fn result_file_name_shape() {
        assert_eq!(
            result_file_name("0123456789abcdef0123456789abcdef"),
            "helper-result-0123456789abcdef0123456789abcdef.txt"
        );
    }

    #[test]
    fn render_parse_round_trip_ok() {
        let r = HelperResult::Ok;
        assert_eq!(render(&r), "ok");
        assert_eq!(parse("ok"), Some(HelperResult::Ok));
        assert_eq!(parse(&render(&r)), Some(r));
    }

    #[test]
    fn render_parse_round_trip_error() {
        let r = HelperResult::Error("something broke".to_owned());
        assert_eq!(render(&r), "error: something broke");
        assert_eq!(parse(&render(&r)), Some(r));
    }

    #[test]
    fn parse_tolerates_trailing_newline_and_empty_detail() {
        assert_eq!(parse("ok\n"), Some(HelperResult::Ok));
        assert_eq!(parse("error:\n"), Some(HelperResult::Error(String::new())));
    }

    #[test]
    fn parse_rejects_unknown_body() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("garbage"), None);
    }
}
