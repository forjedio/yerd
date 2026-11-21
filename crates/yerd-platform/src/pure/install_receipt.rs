//! The install receipt that marks a headless (CLI-only) install.
//!
//! A headless tarball ships `yerd-install-receipt` beside the binaries with the
//! content `headless`. It travels with the binaries through the install script,
//! a manual extract, and every self-update swap, so it is per-install: a
//! desktop app install never sees it. A missing receipt means the package or
//! app install behaviour; a receipt that is present but unreadable or
//! unrecognised is a hard error, never a silent fallback.
//!
//! The edges (`yerdd`, `yerd`, `yerd-helper`) do the read and encode what they
//! saw as a [`ReceiptRead`]; this module only classifies it.

/// File name of the receipt, a sibling of the canonical binaries.
pub const RECEIPT_FILE: &str = "yerd-install-receipt";

/// How this copy of Yerd was installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    /// A desktop app or distro package install (no receipt).
    Package,
    /// A headless tarball install (receipt says `headless`).
    Headless,
}

/// Why the install mode could not be determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReceiptError {
    /// The receipt exists but could not be read, or its content is not known.
    #[error("the install receipt ({RECEIPT_FILE}) is unreadable or unrecognised; reinstall with the install script")]
    Unrecognised,
    /// The running binary's own path could not be resolved.
    #[error("the running binary was replaced on disk; restart it (`yerd restart daemon`) to load the new version")]
    ExeUnresolved,
}

/// What the edge observed when it looked for the receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptRead {
    /// Canonicalizing the running executable failed (e.g. Linux reports
    /// `"<path> (deleted)"` after a rename-over).
    ExeUnresolved,
    /// The receipt file does not exist (`NotFound`).
    Absent,
    /// The receipt exists but reading it failed for any other reason.
    Unreadable,
    /// The receipt's raw content.
    Content(String),
}

/// Parse a receipt's content. Surrounding whitespace is ignored; only the
/// exact lowercase `headless` is recognised.
///
/// # Errors
///
/// [`ReceiptError::Unrecognised`] for any other content.
pub fn parse(content: &str) -> Result<InstallMode, ReceiptError> {
    match content.trim() {
        "headless" => Ok(InstallMode::Headless),
        _ => Err(ReceiptError::Unrecognised),
    }
}

/// Turn what the edge saw into an install mode.
///
/// # Errors
///
/// [`ReceiptError::ExeUnresolved`] when the executable path was unresolvable,
/// [`ReceiptError::Unrecognised`] for an unreadable or unknown receipt.
pub fn classify(read: ReceiptRead) -> Result<InstallMode, ReceiptError> {
    match read {
        ReceiptRead::ExeUnresolved => Err(ReceiptError::ExeUnresolved),
        ReceiptRead::Absent => Ok(InstallMode::Package),
        ReceiptRead::Unreadable => Err(ReceiptError::Unrecognised),
        ReceiptRead::Content(content) => parse(&content),
    }
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
    fn classify_covers_every_read_outcome() {
        let content = |s: &str| ReceiptRead::Content(s.to_owned());
        let cases: Vec<(ReceiptRead, Result<InstallMode, ReceiptError>)> = vec![
            (ReceiptRead::Absent, Ok(InstallMode::Package)),
            (ReceiptRead::ExeUnresolved, Err(ReceiptError::ExeUnresolved)),
            (ReceiptRead::Unreadable, Err(ReceiptError::Unrecognised)),
            (content("headless"), Ok(InstallMode::Headless)),
            (content("headless\n"), Ok(InstallMode::Headless)),
            (content("  headless  "), Ok(InstallMode::Headless)),
            (content(""), Err(ReceiptError::Unrecognised)),
            (content("gui"), Err(ReceiptError::Unrecognised)),
            (content("HEADLESS"), Err(ReceiptError::Unrecognised)),
        ];
        for (read, expected) in cases {
            let label = format!("{read:?}");
            assert_eq!(classify(read), expected, "{label}");
        }
    }

    #[test]
    fn error_messages_carry_the_user_hint() {
        assert!(ReceiptError::Unrecognised
            .to_string()
            .contains("reinstall with the install script"));
        assert_eq!(
            ReceiptError::ExeUnresolved.to_string(),
            "the running binary was replaced on disk; restart it (`yerd restart daemon`) to \
             load the new version"
        );
    }
}
