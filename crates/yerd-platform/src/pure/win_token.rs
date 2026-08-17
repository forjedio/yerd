//! Pure parse of `whoami /groups` output for an elevated-integrity token.
//!
//! Compiled on every OS so Linux/macOS CI table-tests it too. No I/O: the
//! caller spawns `whoami.exe` by absolute path and hands the CSV here.
//!
//! A UAC-elevated process always runs at **High** integrity (`S-1-16-12288`);
//! the built-in SYSTEM/service context runs at **System** integrity
//! (`S-1-16-16384`); a normal, non-elevated process is **Medium**
//! (`S-1-16-8192`). Matching the two elevated mandatory-integrity SIDs is a
//! faithful proxy for `GetTokenInformation(TokenElevation)` for every case Yerd
//! meets, without pulling `unsafe` FFI into the security boundary.

/// The High mandatory-integrity level SID (UAC-elevated processes).
const HIGH_INTEGRITY_SID: &str = "S-1-16-12288";
/// The System mandatory-integrity level SID (SYSTEM / services).
const SYSTEM_INTEGRITY_SID: &str = "S-1-16-16384";

/// Whether `whoami /groups` output shows a High- or System-integrity token.
///
/// A plain substring test on the quoted CSV is sufficient: mandatory-integrity
/// SIDs are locale-independent literals, and neither is a substring of the
/// Medium SID (`S-1-16-8192`) or of any other integrity level.
#[must_use]
pub fn csv_has_elevated_integrity(whoami_groups_csv: &str) -> bool {
    whoami_groups_csv.contains(HIGH_INTEGRITY_SID)
        || whoami_groups_csv.contains(SYSTEM_INTEGRITY_SID)
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
    fn high_integrity_is_elevated() {
        let csv = "\"Mandatory Label\\High Mandatory Level\",\"Label\",\"S-1-16-12288\",\"\"\n";
        assert!(csv_has_elevated_integrity(csv));
    }

    #[test]
    fn system_integrity_is_elevated() {
        let csv = "\"Mandatory Label\\System Mandatory Level\",\"Label\",\"S-1-16-16384\",\"\"\n";
        assert!(csv_has_elevated_integrity(csv));
    }

    #[test]
    fn medium_integrity_is_not_elevated() {
        let csv = "\"Mandatory Label\\Medium Mandatory Level\",\"Label\",\"S-1-16-8192\",\"\"\n";
        assert!(!csv_has_elevated_integrity(csv));
    }

    #[test]
    fn garbage_and_empty_are_not_elevated() {
        assert!(!csv_has_elevated_integrity(""));
        assert!(!csv_has_elevated_integrity("no sids here at all"));
        assert!(!csv_has_elevated_integrity("S-1-5-21-1-2-3-1001"));
    }
}
