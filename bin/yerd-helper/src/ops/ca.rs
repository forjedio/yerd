//! `install-ca` and `uninstall-ca` for Linux + macOS.

use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use yerd_platform::pure::pem_match;
use yerd_platform::CaFingerprint;

#[cfg(target_os = "macos")]
use crate::error::CommandReason;
use crate::error::HelperError;
#[cfg(target_os = "linux")]
use crate::error::ValidationReason;
#[cfg(target_os = "linux")]
use crate::ops::atomic_write;
use crate::ops::run_command;
use crate::validate;

/// Anchor directories Yerd searches on Linux. Order matches the
/// distro family precedence.
#[cfg(target_os = "linux")]
const ANCHOR_DIRS: &[&str] = &[
    "/usr/local/share/ca-certificates", // Debian/Ubuntu/Alpine
    "/etc/pki/ca-trust/source/anchors", // RHEL/Fedora/CentOS
    "/etc/ca-certificates/trust-source/anchors", // Arch
];

/// On Linux, determine the post-install command from the anchor dir.
/// Pure function — table-tested.
#[cfg(target_os = "linux")]
#[must_use]
pub fn linux_post_install_cmd(anchor_dir: &Path) -> Option<(&'static str, Vec<&'static str>)> {
    let s = anchor_dir.to_str()?;
    Some(match s {
        "/usr/local/share/ca-certificates" => ("update-ca-certificates", vec![]),
        "/etc/pki/ca-trust/source/anchors" => ("update-ca-trust", vec!["extract"]),
        "/etc/ca-certificates/trust-source/anchors" => ("trust", vec!["extract-compat"]),
        _ => return None,
    })
}

#[cfg(target_os = "linux")]
fn pick_anchor_dir() -> Result<PathBuf, HelperError> {
    for dir in ANCHOR_DIRS {
        if Path::new(dir).is_dir() {
            return Ok(PathBuf::from(dir));
        }
    }
    Err(HelperError::Validation {
        reason: ValidationReason::NoAnchorDir,
    })
}

/// Anchor filename: `yerd-<first-16-hex-of-fp>.crt`. The full
/// fingerprint is the security boundary; the filename is just a
/// human-readable, unique-in-practice tag.
#[cfg(target_os = "linux")]
fn anchor_filename(fp: &CaFingerprint) -> String {
    let full = hex::encode(fp.as_bytes());
    let short: String = full.chars().take(16).collect();
    format!("yerd-{short}.crt")
}

// ---- install-ca -----------------------------------------------------

#[cfg(target_os = "linux")]
pub fn install_ca(pem_path: &Path, fp: &CaFingerprint) -> Result<(), HelperError> {
    validate::require_existing_file(pem_path)?;
    let der = validate::require_pem_matches_fingerprint(pem_path, fp)?;
    let anchor_dir = pick_anchor_dir()?;
    let dest = anchor_dir.join(anchor_filename(fp));
    // Re-emit the PEM from the validated DER to avoid copying anything
    // the daemon prepended outside the single CERTIFICATE block.
    let pem_text = pem_match::der_to_pem(&der);
    atomic_write(&dest, pem_text.as_bytes(), true)?;
    let (tool, args) = linux_post_install_cmd(&anchor_dir).ok_or(HelperError::Validation {
        reason: ValidationReason::NoAnchorDir,
    })?;
    run_command(tool, tool, args).map(|_| ())
}

#[cfg(target_os = "macos")]
pub fn install_ca(pem_path: &Path, fp: &CaFingerprint) -> Result<(), HelperError> {
    validate::require_existing_file(pem_path)?;
    let _der = validate::require_pem_matches_fingerprint(pem_path, fp)?;
    // `security add-trusted-cert` (mkcert's approach: -d admin domain + -r
    // trustRoot) writes the certificate *and* its trust setting atomically —
    // but only when the cert is not already in the keychain. If the cert is
    // already present it exits non-zero with errSecDuplicateItem and SKIPS the
    // trust-settings write, leaving the anchor present-but-untrusted (Chrome:
    // ERR_CERT_AUTHORITY_INVALID) while still appearing installed. So delete any
    // existing copy first, guaranteeing the add below applies the trust setting.
    if macos_system_keychain_contains(fp)? {
        let fp_upper = hex::encode_upper(fp.as_bytes());
        run_command(
            "security",
            "/usr/bin/security",
            [
                "delete-certificate",
                "-Z",
                &fp_upper,
                "/Library/Keychains/System.keychain",
            ],
        )?;
    }
    run_command(
        "security",
        "/usr/bin/security",
        [
            "add-trusted-cert",
            "-d",
            "-r",
            "trustRoot",
            "-k",
            "/Library/Keychains/System.keychain",
            pem_path.to_string_lossy().as_ref(),
        ],
    )
    .map(|_| ())
}

// ---- uninstall-ca ---------------------------------------------------

#[cfg(target_os = "linux")]
pub fn uninstall_ca(fp: &CaFingerprint) -> Result<(), HelperError> {
    let anchor_dir = pick_anchor_dir()?;
    let entries = std::fs::read_dir(&anchor_dir).map_err(|source| HelperError::Io {
        path: anchor_dir.clone(),
        source,
    })?;

    let mut blobs: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("crt") {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|source| HelperError::Io {
            path: path.clone(),
            source,
        })?;
        blobs.push((path, bytes));
    }

    let matched = pem_match::find_by_fingerprint(&blobs, fp.as_bytes()).map_err(|_bad_path| {
        HelperError::Validation {
            reason: ValidationReason::PemParseFailed,
        }
    })?;

    let Some(m) = matched else {
        // Idempotent: nothing to remove.
        return Ok(());
    };

    std::fs::remove_file(&m.path).map_err(|source| HelperError::Io {
        path: m.path.clone(),
        source,
    })?;

    let (tool, args) = linux_post_install_cmd(&anchor_dir).ok_or(HelperError::Validation {
        reason: ValidationReason::NoAnchorDir,
    })?;
    run_command(tool, tool, args).map(|_| ())
}

#[cfg(target_os = "macos")]
pub fn uninstall_ca(fp: &CaFingerprint) -> Result<(), HelperError> {
    // Idempotent: if the fingerprint isn't in the System keychain there's
    // nothing to remove.
    if !macos_system_keychain_contains(fp)? {
        return Ok(());
    }
    let fp_upper = hex::encode_upper(fp.as_bytes());
    run_command(
        "security",
        "/usr/bin/security",
        [
            "delete-certificate",
            "-Z",
            &fp_upper,
            "/Library/Keychains/System.keychain",
        ],
    )
    .map(|_| ())
}

/// Is the CA with fingerprint `fp` present in the System keychain?
///
/// Runs `security find-certificate -Z -a` (which lists every cert with its
/// SHA-256 hash) and matches the fingerprint. `security` exits non-zero when no
/// certs match the query, which we treat as "absent". This is the shared
/// presence probe behind both `install_ca`'s benign-duplicate tolerance and
/// `uninstall_ca`'s idempotency.
#[cfg(target_os = "macos")]
fn macos_system_keychain_contains(fp: &CaFingerprint) -> Result<bool, HelperError> {
    let probe = run_command(
        "security",
        "/usr/bin/security",
        [
            "find-certificate",
            "-Z",
            "-a",
            "/Library/Keychains/System.keychain",
        ],
    );
    let fp_upper = hex::encode_upper(fp.as_bytes());
    match probe {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            Ok(macos_find_certificate_contains(&stdout, &fp_upper))
        }
        Err(HelperError::Command {
            reason: CommandReason::NonZero(_),
            ..
        }) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Pure: does `find-certificate -Z` stdout contain the uppercase
/// fingerprint? `security` emits `SHA-256 hash: <64hex>` lines per
/// certificate.
#[cfg(target_os = "macos")]
#[must_use]
pub fn macos_find_certificate_contains(stdout: &str, fp_upper: &str) -> bool {
    stdout.lines().any(|line| {
        line.trim()
            .strip_prefix("SHA-256 hash:")
            .is_some_and(|rest| rest.trim().eq_ignore_ascii_case(fp_upper))
    })
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

    #[cfg(target_os = "linux")]
    #[test]
    fn anchor_filename_uses_first_16_hex() {
        let fp = CaFingerprint::new([0xAB; 32]);
        let name = anchor_filename(&fp);
        assert_eq!(name, "yerd-abababababababab.crt");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_post_install_cmd_debian() {
        let (tool, args) =
            linux_post_install_cmd(Path::new("/usr/local/share/ca-certificates")).unwrap();
        assert_eq!(tool, "update-ca-certificates");
        assert!(args.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_post_install_cmd_rhel() {
        let (tool, args) =
            linux_post_install_cmd(Path::new("/etc/pki/ca-trust/source/anchors")).unwrap();
        assert_eq!(tool, "update-ca-trust");
        assert_eq!(args, vec!["extract"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_post_install_cmd_arch() {
        let (tool, args) =
            linux_post_install_cmd(Path::new("/etc/ca-certificates/trust-source/anchors")).unwrap();
        assert_eq!(tool, "trust");
        assert_eq!(args, vec!["extract-compat"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_post_install_cmd_unknown_returns_none() {
        assert!(linux_post_install_cmd(Path::new("/etc/bogus")).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn find_certificate_contains_matches_when_present() {
        let stdout = "keychain: \"/Library/Keychains/System.keychain\"\n\
                      SHA-256 hash: ABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABAB\n\
                      ...\n";
        assert!(macos_find_certificate_contains(stdout, &"AB".repeat(32)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn find_certificate_contains_false_when_absent() {
        let stdout = "keychain: \"/Library/Keychains/System.keychain\"\n\
                      SHA-256 hash: CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC\n";
        assert!(!macos_find_certificate_contains(stdout, &"AB".repeat(32)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn find_certificate_contains_case_insensitive() {
        let stdout =
            "SHA-256 hash: abababababababababababababababababababababababababababababababab\n";
        assert!(macos_find_certificate_contains(stdout, &"AB".repeat(32)));
    }
}
