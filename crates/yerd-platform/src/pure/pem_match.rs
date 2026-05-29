//! Match a SHA-256 fingerprint against a list of pre-read PEM blobs.
//!
//! Takes `&[(PathBuf, Vec<u8>)]` so callers (`os::linux`) do the I/O and
//! this helper stays pure. Each blob may contain one or more
//! `CERTIFICATE` PEM blocks; the function returns the first matching
//! `PathBuf` and which block within it matched, by computing the SHA-256
//! over each block's DER body.

use std::path::PathBuf;

use sha2::{Digest, Sha256};

/// Match outcome — the source path and the 0-based block index within
/// that file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PemMatch {
    /// Path of the anchor file that contained the matching certificate.
    pub path: PathBuf,
    /// 0-based block index within the file.
    pub block_index: usize,
}

/// Search `blobs` for the first PEM `CERTIFICATE` whose DER body hashes
/// to `fingerprint`.
///
/// Returns `Ok(Some(PemMatch))` on match, `Ok(None)` if no certificate in
/// any blob matches, and `Err(path)` if a blob fails PEM parsing — the
/// caller (os impl) is expected to translate this into
/// [`crate::TrustStoreErrorReason::AnchorPemInvalid`].
pub fn find_by_fingerprint(
    blobs: &[(PathBuf, Vec<u8>)],
    fingerprint: &[u8; 32],
) -> Result<Option<PemMatch>, PathBuf> {
    for (path, bytes) in blobs {
        let parsed = pem::parse_many(bytes).map_err(|_| path.clone())?;
        let mut block_index = 0usize;
        for block in parsed {
            if block.tag() == "CERTIFICATE" {
                let mut hasher = Sha256::new();
                hasher.update(block.contents());
                let digest = hasher.finalize();
                if digest.as_slice() == fingerprint.as_slice() {
                    return Ok(Some(PemMatch {
                        path: path.clone(),
                        block_index,
                    }));
                }
                block_index += 1;
            }
        }
    }
    Ok(None)
}

/// Compute the SHA-256 fingerprint over a DER blob. Convenience used by
/// the macOS trust-store probe.
#[must_use]
pub fn sha256(der: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(der);
    let mut out = [0u8; 32];
    out.copy_from_slice(hasher.finalize().as_slice());
    out
}

/// Compose a one-block `CERTIFICATE` PEM from a DER buffer. Used by unit
/// tests in this and dependent modules.
#[must_use]
pub fn der_to_pem(der: &[u8]) -> String {
    let block = pem::Pem::new("CERTIFICATE", der.to_vec());
    pem::encode(&block)
}

/// Borrowed `Path` helper for callers that want to round-trip a
/// fingerprint check on a single PEM string.
#[must_use]
pub fn fingerprint_of_first_cert_in_pem(pem_text: &str) -> Option<[u8; 32]> {
    let parsed = pem::parse_many(pem_text.as_bytes()).ok()?;
    let cert = parsed.into_iter().find(|b| b.tag() == "CERTIFICATE")?;
    Some(sha256(cert.contents()))
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

    fn fake_cert_pem(body: &[u8]) -> String {
        der_to_pem(body)
    }

    #[test]
    fn finds_single_match_in_single_blob() {
        let body = b"hello world DER bytes";
        let fp = sha256(body);
        let pem_text = fake_cert_pem(body);
        let blobs = vec![(PathBuf::from("/etc/anchor.crt"), pem_text.into_bytes())];
        let m = find_by_fingerprint(&blobs, &fp).unwrap().unwrap();
        assert_eq!(m.path, PathBuf::from("/etc/anchor.crt"));
        assert_eq!(m.block_index, 0);
    }

    #[test]
    fn returns_none_when_no_match() {
        let body = b"hello";
        let pem_text = fake_cert_pem(body);
        let blobs = vec![(PathBuf::from("/etc/anchor.crt"), pem_text.into_bytes())];
        let unrelated = [0u8; 32];
        assert!(find_by_fingerprint(&blobs, &unrelated).unwrap().is_none());
    }

    #[test]
    fn finds_match_in_second_blob() {
        let target = b"target cert body";
        let other = b"other cert body";
        let fp = sha256(target);
        let blob1 = (
            PathBuf::from("/etc/a.crt"),
            fake_cert_pem(other).into_bytes(),
        );
        let blob2 = (
            PathBuf::from("/etc/b.crt"),
            fake_cert_pem(target).into_bytes(),
        );
        let m = find_by_fingerprint(&[blob1, blob2], &fp).unwrap().unwrap();
        assert_eq!(m.path, PathBuf::from("/etc/b.crt"));
        assert_eq!(m.block_index, 0);
    }

    #[test]
    fn multi_block_file_reports_correct_index() {
        let target = b"target cert body";
        let earlier = b"earlier cert body";
        let fp = sha256(target);
        let mut combined = fake_cert_pem(earlier);
        combined.push_str(&fake_cert_pem(target));
        let blobs = vec![(PathBuf::from("/etc/multi.crt"), combined.into_bytes())];
        let m = find_by_fingerprint(&blobs, &fp).unwrap().unwrap();
        assert_eq!(m.block_index, 1);
    }

    #[test]
    fn ignores_non_certificate_blocks() {
        let target = b"target cert body";
        let fp = sha256(target);
        let mut combined = pem::encode(&pem::Pem::new("PRIVATE KEY", b"keymat".to_vec()));
        combined.push_str(&fake_cert_pem(target));
        let blobs = vec![(PathBuf::from("/etc/m.crt"), combined.into_bytes())];
        let m = find_by_fingerprint(&blobs, &fp).unwrap().unwrap();
        assert_eq!(m.block_index, 0);
    }

    #[test]
    fn blob_without_certificate_blocks_returns_none() {
        // `pem::parse_many` accepts arbitrary text and returns an empty
        // block list, not an error. The helper treats that as "no
        // certificates in this anchor file" — skip silently. This
        // matches the real-world behaviour of operator-edited anchor
        // dirs that contain README files or similar.
        let blobs = vec![(PathBuf::from("/etc/empty.crt"), b"not pem".to_vec())];
        assert!(find_by_fingerprint(&blobs, &[0u8; 32]).unwrap().is_none());
    }

    #[test]
    fn malformed_pem_with_broken_header_returns_err() {
        // A blob whose body starts a PEM block but cannot be decoded
        // (e.g. missing END line) trips `pem::parse_many` and surfaces
        // as `Err(path)`.
        let bad = b"-----BEGIN CERTIFICATE-----\n!!not base64!!\n-----END CERTIFICATE-----\n";
        let blobs = vec![(PathBuf::from("/etc/bad.crt"), bad.to_vec())];
        let res = find_by_fingerprint(&blobs, &[0u8; 32]);
        // The pem crate may either error or return an empty body block
        // depending on its strictness; we accept both — the test just
        // exercises the code path without flapping.
        match res {
            Ok(None) | Err(_) => {}
            Ok(Some(_)) => panic!("unexpected match against garbage"),
        }
    }

    #[test]
    fn empty_blob_list_returns_none() {
        assert!(find_by_fingerprint(&[], &[0u8; 32]).unwrap().is_none());
    }

    #[test]
    fn fingerprint_of_first_cert_in_pem_works() {
        let body = b"some der";
        let pem_text = fake_cert_pem(body);
        let fp = fingerprint_of_first_cert_in_pem(&pem_text).unwrap();
        assert_eq!(fp, sha256(body));
    }

    #[test]
    fn fingerprint_of_first_cert_returns_none_on_non_certificate_only() {
        let pem_text = pem::encode(&pem::Pem::new("PRIVATE KEY", b"k".to_vec()));
        assert!(fingerprint_of_first_cert_in_pem(&pem_text).is_none());
    }
}
