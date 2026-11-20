//! Read the install receipt beside the running binary.
//!
//! The edge half of [`crate::pure::install_receipt`]: it resolves the canonical
//! directory of the running executable, reads the receipt there, and encodes
//! what it saw as a [`ReceiptRead`] for the pure classifier. Shared by `yerdd`
//! (artifact selection) and `yerd` (the applier and local commands) so both
//! read the receipt the same way.

use std::path::{Path, PathBuf};

use crate::pure::install_receipt::{self, InstallMode, ReceiptError, ReceiptRead, RECEIPT_FILE};

/// The running executable's path, as [`std::env::current_exe`] but surviving a
/// rename-over: Linux then reports `"<path> (deleted)"`, so when that path is
/// gone and the launch path (the suffix stripped) holds a file, that file is
/// returned. This is what an update or package upgrade leaves behind, and it is
/// the binary a restart should load.
///
/// # Errors
///
/// Whatever [`std::env::current_exe`] returns.
pub fn current_exe() -> std::io::Result<PathBuf> {
    std::env::current_exe().map(resolve_reported_exe)
}

fn resolve_reported_exe(reported: PathBuf) -> PathBuf {
    if reported.exists() {
        return reported;
    }
    install_receipt::strip_deleted_suffix(&reported)
        .filter(|launch| launch.is_file())
        .unwrap_or(reported)
}

/// The canonical directory holding the running executable (see
/// [`current_exe`]), or `None` when the path cannot be resolved, e.g. the
/// binary was deleted and nothing replaced it.
#[must_use]
pub fn current_exe_dir() -> Option<PathBuf> {
    let exe = current_exe().and_then(std::fs::canonicalize).ok()?;
    exe.parent().map(Path::to_path_buf)
}

/// Read the receipt in `dir`: [`ReceiptRead::Absent`] only for `NotFound`,
/// [`ReceiptRead::Unreadable`] for any other error.
#[must_use]
pub fn read_receipt_in(dir: &Path) -> ReceiptRead {
    match std::fs::read_to_string(dir.join(RECEIPT_FILE)) {
        Ok(content) => ReceiptRead::Content(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ReceiptRead::Absent,
        Err(_) => ReceiptRead::Unreadable,
    }
}

/// The install mode of the running binary, from the receipt beside its
/// canonical executable.
///
/// # Errors
///
/// [`ReceiptError::ExeUnresolved`] when the executable path cannot be resolved,
/// [`ReceiptError::Unrecognised`] for an unreadable or unknown receipt.
pub fn detect() -> Result<InstallMode, ReceiptError> {
    let read = current_exe_dir().map_or(ReceiptRead::ExeUnresolved, |dir| read_receipt_in(&dir));
    install_receipt::classify(read)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn read_receipt_in_reports_absent_content_and_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_receipt_in(tmp.path()), ReceiptRead::Absent);

        std::fs::write(tmp.path().join(RECEIPT_FILE), "headless\n").unwrap();
        assert_eq!(
            read_receipt_in(tmp.path()),
            ReceiptRead::Content("headless\n".to_owned())
        );

        let dir_as_receipt = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir_as_receipt.path().join(RECEIPT_FILE)).unwrap();
        assert_eq!(
            read_receipt_in(dir_as_receipt.path()),
            ReceiptRead::Unreadable
        );
    }

    #[test]
    fn resolve_reported_exe_follows_a_renamed_over_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let launch = tmp.path().join("yerdd");
        let reported = tmp.path().join("yerdd (deleted)");

        assert_eq!(resolve_reported_exe(reported.clone()), reported);

        std::fs::write(&launch, b"new").unwrap();
        assert_eq!(resolve_reported_exe(reported.clone()), launch);
        assert_eq!(resolve_reported_exe(launch.clone()), launch);

        std::fs::write(&reported, b"literal").unwrap();
        assert_eq!(resolve_reported_exe(reported.clone()), reported);

        let dir_only = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir_only.path().join("yerdd")).unwrap();
        let gone = dir_only.path().join("yerdd (deleted)");
        assert_eq!(resolve_reported_exe(gone.clone()), gone);
    }

    #[test]
    fn test_binary_has_no_receipt() {
        assert_eq!(detect(), Ok(InstallMode::Package));
    }
}
