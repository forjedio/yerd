//! Artifact selection + verification for self-update.
//!
//! Given a resolved [`crate::ReleaseMeta`] and the running [`Platform`], pick the
//! right downloadable artifact (the macOS `.app.tar.gz` or the Linux `.deb`),
//! its detached minisign `.sig`, and the `SHA256SUMS` manifest. Verification is
//! pure: it operates on already-downloaded bytes.

use sha2::{Digest, Sha256};

use crate::{Asset, ReleaseMeta};

/// The minisign public key whose secret half signs release artifacts.
pub const UPDATE_PUBLIC_KEY: &str = "RWRXUQIpU8uZ3B6SV3yFsK3+aAWZX+efytjc8F+8PTuViL8/nNPsQxpi";

/// The host platform an artifact targets. Decoupled from `cfg!` so selection is
/// testable for every platform from any build; the daemon passes
/// [`Platform::current`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Apple Silicon macOS — the `.app.tar.gz` bundle.
    MacOsAarch64,
    /// Intel macOS — no artifact is published (Apple-Silicon-only for MVP).
    MacOsX86_64,
    /// `x86_64` Linux — the `.deb` package.
    LinuxX86_64,
    /// `aarch64` Linux — the arm64 `.deb` package.
    LinuxAarch64,
    /// Any platform without a published self-update artifact.
    Unsupported,
}

impl Platform {
    /// The platform this binary was built for. `Unsupported` for anything we
    /// don't publish a self-update artifact for (incl. Windows and other arches).
    #[must_use]
    pub fn current() -> Self {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            Self::MacOsAarch64
        }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            Self::MacOsX86_64
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            Self::LinuxX86_64
        }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            Self::LinuxAarch64
        }
        #[cfg(not(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "aarch64"),
        )))]
        {
            Self::Unsupported
        }
    }
}

/// The kind of artifact, which drives how the applier installs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// A gzip'd tar of the macOS `.app` bundle (swapped into `/Applications`).
    AppTarGz,
    /// A Debian package (reinstalled via `dpkg -i`).
    Deb,
}

/// A fully-resolved download set for one platform: the artifact, its detached
/// signature, and the checksum manifest. Borrows from the [`ReleaseMeta`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactSelection<'a> {
    /// The primary artifact (`.app.tar.gz` / `.deb`).
    pub artifact: &'a Asset,
    /// The detached minisign signature (`<artifact>.sig`).
    pub signature: &'a Asset,
    /// The `SHA256SUMS` manifest covering the artifact.
    pub checksums: &'a Asset,
    /// What kind of artifact it is.
    pub kind: ArtifactKind,
}

/// Why [`select_asset`] could not resolve a download set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetError {
    /// No artifact is published for this platform (e.g. Intel macOS, Windows).
    NoArtifactForPlatform(Platform),
    /// The artifact is present but its `.sig` is missing.
    MissingSignature(String),
    /// No `SHA256SUMS` manifest is attached to the release.
    MissingChecksums,
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoArtifactForPlatform(p) => {
                write!(f, "no self-update artifact published for {p:?}")
            }
            Self::MissingSignature(name) => write!(f, "missing signature for {name}"),
            Self::MissingChecksums => f.write_str("release has no SHA256SUMS manifest"),
        }
    }
}

impl std::error::Error for AssetError {}

/// Pick the artifact + signature + checksums for `platform` from `release`.
///
/// Selection is by filename convention (the names the release workflow emits):
/// the macOS artifact ends `.app.tar.gz` and is arch-tagged; the Linux artifact
/// ends `.deb` and is arch-tagged (`x86_64` or `arm64`); the signature is
/// `<artifact>.sig`; the manifest is named `SHA256SUMS`. Intel macOS / unsupported
/// platforms return [`AssetError::NoArtifactForPlatform`] rather than mis-selecting.
pub fn select_asset(
    release: &ReleaseMeta,
    platform: Platform,
) -> Result<ArtifactSelection<'_>, AssetError> {
    let (kind, matches): (ArtifactKind, fn(&str) -> bool) = match platform {
        Platform::MacOsAarch64 => (ArtifactKind::AppTarGz, is_macos_aarch64_artifact),
        Platform::LinuxX86_64 => (ArtifactKind::Deb, is_linux_x86_64_artifact),
        Platform::LinuxAarch64 => (ArtifactKind::Deb, is_linux_aarch64_artifact),
        p @ (Platform::MacOsX86_64 | Platform::Unsupported) => {
            return Err(AssetError::NoArtifactForPlatform(p));
        }
    };

    let artifact = release
        .assets
        .iter()
        .find(|a| matches(&a.name))
        .ok_or(AssetError::NoArtifactForPlatform(platform))?;

    let sig_name = format!("{}.sig", artifact.name);
    let signature = release
        .assets
        .iter()
        .find(|a| a.name == sig_name)
        .ok_or_else(|| AssetError::MissingSignature(artifact.name.clone()))?;

    let checksums = release
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS")
        .ok_or(AssetError::MissingChecksums)?;

    Ok(ArtifactSelection {
        artifact,
        signature,
        checksums,
        kind,
    })
}

// The release workflow controls these filenames and their exact (lowercase)
// extensions, so a case-sensitive suffix check is correct here.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn is_macos_aarch64_artifact(name: &str) -> bool {
    name.ends_with(".app.tar.gz")
        && (name.contains("AppleSilicon") || name.contains("aarch64") || name.contains("arm64"))
}

#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn is_linux_x86_64_artifact(name: &str) -> bool {
    name.ends_with(".deb") && (name.contains("x86_64") || name.contains("amd64"))
}

// The published arm64 .deb is named `Yerd_Linux_Arm64_*.deb` (capital "Arm64"), so
// match case-insensitively — a lowercase `contains("arm64")` would miss it.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn is_linux_aarch64_artifact(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".deb") && (lower.contains("aarch64") || lower.contains("arm64"))
}

/// Why verification failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// The artifact's SHA-256 did not match the manifest entry.
    ChecksumMismatch,
    /// The `SHA256SUMS` manifest had no line for the artifact filename.
    ChecksumMissing,
    /// The embedded public key string was not a valid minisign key.
    BadPublicKey,
    /// The `.sig` content was not a valid minisign signature.
    BadSignature,
    /// The signature did not verify against the public key + bytes.
    SignatureMismatch,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::ChecksumMismatch => "artifact SHA-256 does not match SHA256SUMS",
            Self::ChecksumMissing => "artifact not listed in SHA256SUMS",
            Self::BadPublicKey => "embedded update public key is invalid",
            Self::BadSignature => "artifact signature is malformed",
            Self::SignatureMismatch => "artifact signature does not verify",
        };
        f.write_str(s)
    }
}

impl std::error::Error for VerifyError {}

/// Lowercase hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(digest)
}

/// Find the expected SHA-256 (lowercase hex) for `filename` in a `SHA256SUMS`
/// body. Accepts the standard `<hex>␠␠<name>` / `<hex>␠*<name>` line formats and
/// tolerates a leading `*` or `./` on the name.
#[must_use]
pub fn sha256_for<'a>(sums: &'a str, filename: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hex = parts.next()?;
        let name = parts.next()?;
        let name = name.strip_prefix('*').unwrap_or(name);
        let name = name.strip_prefix("./").unwrap_or(name);
        (name == filename).then_some(hex)
    })
}

/// Verify `bytes` against the `SHA256SUMS` entry for `filename`.
pub fn verify_sha256(bytes: &[u8], sums: &str, filename: &str) -> Result<(), VerifyError> {
    let expected = sha256_for(sums, filename).ok_or(VerifyError::ChecksumMissing)?;
    if sha256_hex(bytes).eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(VerifyError::ChecksumMismatch)
    }
}

/// Verify a detached minisign `signature` over `bytes` using `public_key_b64`.
///
/// Requires a **prehashed** signature (`minisign -H`, which `tauri signer` and
/// the modern `minisign` default produce) — legacy ed25519 signatures are
/// rejected. The release workflow signs with prehashing.
pub fn verify_minisign(
    public_key_b64: &str,
    signature: &str,
    bytes: &[u8],
) -> Result<(), VerifyError> {
    let pk = minisign_verify::PublicKey::from_base64(public_key_b64)
        .map_err(|_| VerifyError::BadPublicKey)?;
    let sig =
        minisign_verify::Signature::decode(signature).map_err(|_| VerifyError::BadSignature)?;
    pk.verify(bytes, &sig, false)
        .map_err(|_| VerifyError::SignatureMismatch)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::case_sensitive_file_extension_comparisons
)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.to_string(),
            url: format!("https://example.test/{name}"),
            size: 1,
        }
    }

    fn release_with(names: &[&str]) -> ReleaseMeta {
        ReleaseMeta {
            version: semver::Version::parse("2.0.2").unwrap(),
            tag: "v2.0.2".into(),
            prerelease: false,
            assets: names.iter().map(|n| asset(n)).collect(),
            notes: None,
        }
    }

    // ── Known-good minisign fixture (the `minisign-verify` crate's test vector:
    //    a prehashed signature of the bytes `b"test"`). ──
    const FIXTURE_PUBKEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const FIXTURE_SIG: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1556193335\tfile:test\ny/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==";

    #[test]
    fn selects_macos_aarch64_app_tarball() {
        let r = release_with(&[
            "Yerd_MacOS_AppleSilicon_v2-0-2.app.tar.gz",
            "Yerd_MacOS_AppleSilicon_v2-0-2.app.tar.gz.sig",
            "Yerd_MacOS_AppleSilicon_v2-0-2.dmg",
            "SHA256SUMS",
        ]);
        let sel = select_asset(&r, Platform::MacOsAarch64).unwrap();
        assert_eq!(sel.kind, ArtifactKind::AppTarGz);
        assert!(sel.artifact.name.ends_with(".app.tar.gz"));
        assert!(sel.signature.name.ends_with(".app.tar.gz.sig"));
        assert_eq!(sel.checksums.name, "SHA256SUMS");
    }

    #[test]
    fn selects_linux_x86_64_deb() {
        let r = release_with(&[
            "Yerd_Linux_x86_64_v2-0-2.deb",
            "Yerd_Linux_x86_64_v2-0-2.deb.sig",
            "SHA256SUMS",
        ]);
        let sel = select_asset(&r, Platform::LinuxX86_64).unwrap();
        assert_eq!(sel.kind, ArtifactKind::Deb);
        assert!(sel.artifact.name.ends_with(".deb"));
    }

    #[test]
    fn selects_linux_aarch64_deb() {
        // Capital "Arm64" — the real published name; locks in case-insensitive matching.
        let r = release_with(&[
            "Yerd_Linux_Arm64_v2-0-2.deb",
            "Yerd_Linux_Arm64_v2-0-2.deb.sig",
            "SHA256SUMS",
        ]);
        let sel = select_asset(&r, Platform::LinuxAarch64).unwrap();
        assert_eq!(sel.kind, ArtifactKind::Deb);
        assert_eq!(sel.artifact.name, "Yerd_Linux_Arm64_v2-0-2.deb");
    }

    #[test]
    fn linux_arch_matchers_are_disjoint() {
        // arm64 platform must not pick the x86_64 .deb...
        let only_x86 = release_with(&[
            "Yerd_Linux_x86_64_v2-0-2.deb",
            "Yerd_Linux_x86_64_v2-0-2.deb.sig",
            "SHA256SUMS",
        ]);
        assert_eq!(
            select_asset(&only_x86, Platform::LinuxAarch64),
            Err(AssetError::NoArtifactForPlatform(Platform::LinuxAarch64))
        );
        // ...and the x86_64 platform must not pick the arm64 .deb.
        let only_arm = release_with(&[
            "Yerd_Linux_Arm64_v2-0-2.deb",
            "Yerd_Linux_Arm64_v2-0-2.deb.sig",
            "SHA256SUMS",
        ]);
        assert_eq!(
            select_asset(&only_arm, Platform::LinuxX86_64),
            Err(AssetError::NoArtifactForPlatform(Platform::LinuxX86_64))
        );
    }

    #[test]
    fn intel_macos_has_no_artifact() {
        let r = release_with(&["Yerd_MacOS_AppleSilicon_v2-0-2.app.tar.gz", "SHA256SUMS"]);
        assert_eq!(
            select_asset(&r, Platform::MacOsX86_64),
            Err(AssetError::NoArtifactForPlatform(Platform::MacOsX86_64))
        );
    }

    #[test]
    fn missing_signature_is_an_error() {
        let r = release_with(&["Yerd_Linux_x86_64_v2-0-2.deb", "SHA256SUMS"]);
        assert!(matches!(
            select_asset(&r, Platform::LinuxX86_64),
            Err(AssetError::MissingSignature(_))
        ));
    }

    #[test]
    fn missing_checksums_is_an_error() {
        let r = release_with(&[
            "Yerd_Linux_x86_64_v2-0-2.deb",
            "Yerd_Linux_x86_64_v2-0-2.deb.sig",
        ]);
        assert_eq!(
            select_asset(&r, Platform::LinuxX86_64),
            Err(AssetError::MissingChecksums)
        );
    }

    #[test]
    fn sha256_round_trip_and_manifest_lookup() {
        let data = b"hello yerd";
        let hexsum = sha256_hex(data);
        let sums = format!("{hexsum}  Yerd_Linux_x86_64_v2-0-2.deb\nffff  other\n");
        verify_sha256(data, &sums, "Yerd_Linux_x86_64_v2-0-2.deb").unwrap();
        // Tampered bytes fail.
        assert_eq!(
            verify_sha256(b"tampered", &sums, "Yerd_Linux_x86_64_v2-0-2.deb"),
            Err(VerifyError::ChecksumMismatch)
        );
        // Unknown filename fails.
        assert_eq!(
            verify_sha256(data, &sums, "absent.deb"),
            Err(VerifyError::ChecksumMissing)
        );
    }

    #[test]
    fn sha256_manifest_tolerates_star_and_dot_slash() {
        let data = b"x";
        let h = sha256_hex(data);
        assert_eq!(
            sha256_for(&format!("{h} *./name"), "name"),
            Some(h.as_str())
        );
    }

    #[test]
    fn minisign_verifies_good_signature() {
        verify_minisign(FIXTURE_PUBKEY, FIXTURE_SIG, b"test").unwrap();
    }

    #[test]
    fn minisign_rejects_tampered_data() {
        assert_eq!(
            verify_minisign(FIXTURE_PUBKEY, FIXTURE_SIG, b"tampered"),
            Err(VerifyError::SignatureMismatch)
        );
    }

    #[test]
    fn minisign_rejects_wrong_key() {
        // A syntactically-valid but different key → key-id mismatch / verify fail.
        let other = "RWSd1IZw0v2bQ0i4i6kTQ7jHj1xFkfHb9G0Vn8u0kHkP9wXxJ8qXJ0kZ";
        let err = verify_minisign(other, FIXTURE_SIG, b"test").unwrap_err();
        assert!(matches!(
            err,
            VerifyError::BadPublicKey | VerifyError::SignatureMismatch
        ));
    }

    #[test]
    fn minisign_rejects_malformed_signature() {
        assert_eq!(
            verify_minisign(FIXTURE_PUBKEY, "not a signature", b"test"),
            Err(VerifyError::BadSignature)
        );
    }

    #[test]
    fn current_platform_is_known_on_dev_hosts() {
        // On the supported dev/CI hosts this is never Unsupported; elsewhere the
        // assertion is vacuously skipped.
        let p = Platform::current();
        #[cfg(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "aarch64"),
        ))]
        assert_ne!(p, Platform::Unsupported);
        let _ = p;
    }
}
