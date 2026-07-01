//! Test-only helpers shared across daemon unit tests.
//!
//! The signed-`php.json` fetch path (`php_install::fetch_verified_listing`)
//! verifies a **prehashed** minisign signature, so tests that exercise install /
//! update / available-versions need a validly-signed manifest. `minisign-verify`
//! only verifies, so we generate a throwaway keypair and sign at runtime with
//! the `minisign` crate (dev-dependency), handing the generated public key to
//! the code under test.

#![allow(clippy::unwrap_used, clippy::expect_used)]

/// A `php.json` body plus its detached minisign signature and the public key it
/// was signed with. Feed `public_key` to `fetch_verified_listing`, and serve
/// `manifest` / `minisig` from a fake `Downloader`.
pub struct SignedManifest {
    /// Base64 public-key line accepted by `yerd_update::verify_minisign`.
    pub public_key: String,
    /// The `php.json` body.
    pub manifest: String,
    /// The detached `php.json.minisig` file contents.
    pub minisig: String,
}

/// Sign `manifest` with a freshly generated keypair (prehashed, as yerd
/// requires) and return it with its signature and public key. Panics on any
/// crypto error - test-only.
///
/// Before returning, the signature is re-checked with the **production**
/// `verify_minisign` so that if the signing crate ever stops producing a
/// prehashed/non-legacy signature this fails loudly here, rather than turning
/// every downstream test red for an opaque reason.
#[must_use]
pub fn sign_manifest(manifest: &str) -> SignedManifest {
    let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let sig_box = minisign::sign(
        Some(&kp.pk),
        &kp.sk,
        std::io::Cursor::new(manifest.as_bytes()),
        Some("test manifest"),
        Some("yerd test"),
    )
    .unwrap();
    let out = SignedManifest {
        public_key: kp.pk.to_base64(),
        manifest: manifest.to_owned(),
        minisig: sig_box.into_string(),
    };
    yerd_update::verify_minisign(&out.public_key, &out.minisig, out.manifest.as_bytes())
        .expect("freshly signed manifest must verify with the production verifier");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_production_verifier() {
        let s = sign_manifest(r#"{"schema":1,"builds":[]}"#);
        assert!(
            yerd_update::verify_minisign(&s.public_key, &s.minisig, s.manifest.as_bytes()).is_ok()
        );
        assert!(yerd_update::verify_minisign(&s.public_key, &s.minisig, b"tampered").is_err());
    }
}
