//! macOS: trust / untrust the local CA **in-process**, in the **user** trust
//! domain (login keychain).
//!
//! Why in-process and not via `yerd elevate trust` → `yerd-helper`: writing
//! admin/system-domain trust always needs a SecurityAgent dialog, which the
//! osascript-spawned root child can't show (no Window Server session), so that
//! path silently fails. The GUI process *does* have a Window Server session, so
//! `SecTrustSettingsSetTrustSettings` for the **user** domain works, prompts as
//! "Yerd", and needs no root — it touches only `~/Library/Keychains`.
//!
//! Integrity gate: the CA path + fingerprint come from the daemon over IPC
//! (never the webview), and we re-verify the PEM's SHA-256 against that
//! fingerprint on the *exact bytes we import* before trusting. This guards
//! against the on-disk CA being corrupted/swapped; it is not a boundary against
//! a compromised daemon (same uid — out of scope).

use std::path::{Path, PathBuf};

use core_foundation::base::TCFType;
use security_framework::certificate::SecCertificate;
use security_framework::os::macos::import_export::ImportOptions;
use security_framework::os::macos::keychain::SecKeychain;
use security_framework::trust_settings::{Domain, TrustSettings};
use security_framework_sys::base::SecCertificateRef;
use security_framework_sys::trust_settings::{kSecTrustSettingsDomainUser, SecTrustSettingsDomain};
use sha2::{Digest, Sha256};
use yerd_ipc::{Request, Response};
use yerd_platform::{CaFingerprint, TrustStore};

use crate::error::GuiError;

// `security-framework-sys` 2.17 exposes Copy/Set but NOT Remove for trust
// settings, so we declare the system symbol ourselves. It lives in
// `Security.framework`, which is already linked via `security-framework-sys`.
extern "C" {
    fn SecTrustSettingsRemoveTrustSettings(
        cert: SecCertificateRef,
        domain: SecTrustSettingsDomain,
    ) -> core_foundation_sys::base::OSStatus;
}

// OSStatus codes we branch on. Some are not exported by name from
// `security-framework-sys` (interaction-not-allowed, authorization-cancelled),
// so we use literals throughout for consistency.
const ERR_SEC_DUPLICATE_ITEM: i32 = -25299;
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
const ERR_SEC_AUTH_FAILED: i32 = -25293;
const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;
const ERR_SEC_INTERNAL_COMPONENT: i32 = -2070;
const ERR_AUTHORIZATION_CANCELED: i32 = -60006;
const ERR_USER_CANCELED: i32 = -128;

/// Trust the local CA for the current user. Pops a "Yerd" SecurityAgent prompt.
pub async fn trust_ca() -> Result<(), GuiError> {
    let (ca_path, fp) = fetch_facts().await?;
    // The keychain dialog + I/O block, so run off the async runtime.
    tokio::task::spawn_blocking(move || do_trust(&ca_path, &fp))
        .await
        .map_err(|e| GuiError::internal(format!("trust task failed to run: {e}")))?
}

/// Remove the user-domain trust for the local CA. Returns `true` if the CA is
/// *still* effectively trusted afterwards — i.e. a system-wide trust set via the
/// terminal (`sudo yerd elevate trust`) remains, which the GUI cannot remove
/// without root.
pub async fn untrust_ca() -> Result<bool, GuiError> {
    let (ca_path, fp) = fetch_facts().await?;
    tokio::task::spawn_blocking(move || do_untrust(&ca_path, &fp))
        .await
        .map_err(|e| GuiError::internal(format!("untrust task failed to run: {e}")))?
}

/// Fetch the authoritative CA path + fingerprint from the daemon (never the
/// webview), so a compromised webview can't ask us to trust an arbitrary file.
async fn fetch_facts() -> Result<(PathBuf, CaFingerprint), GuiError> {
    match crate::ipc::exchange(&Request::DaemonInfo).await? {
        Response::Info {
            ca_path,
            ca_fingerprint,
            ..
        } => {
            let fp = CaFingerprint::from_hex(&ca_fingerprint)
                .map_err(|_| GuiError::internal("daemon returned an invalid CA fingerprint"))?;
            Ok((ca_path, fp))
        }
        Response::Error { message, .. } => Err(GuiError::unreachable(format!(
            "the yerd daemon could not provide CA info: {message}"
        ))),
        _ => Err(GuiError::unreachable(
            "the yerd daemon did not return CA info; is it running?",
        )),
    }
}

fn do_trust(ca_path: &Path, fp: &CaFingerprint) -> Result<(), GuiError> {
    let der = verified_der(ca_path, fp)?;
    let cert =
        SecCertificate::from_der(&der).map_err(|e| sec_err("read the CA certificate", e.code()))?;

    // Import the verified bytes into the login keychain. A public cert import is
    // silent (no prompt); tolerate the cert already being present.
    let keychain =
        SecKeychain::default().map_err(|e| sec_err("open your login keychain", e.code()))?;
    let mut opts = ImportOptions::new();
    opts.filename("ca.cert.cer").keychain(&keychain);
    if let Err(e) = opts.import(&der) {
        let code = e.code();
        if code != ERR_SEC_DUPLICATE_ITEM {
            return Err(sec_err("add the CA to your login keychain", code));
        }
    }

    // The trust write — this is the call that shows the "Yerd" prompt.
    TrustSettings::new(Domain::User)
        .set_trust_settings_always(&cert)
        .map_err(|e| sec_err("trust the CA", e.code()))
}

fn do_untrust(ca_path: &Path, fp: &CaFingerprint) -> Result<bool, GuiError> {
    let der = verified_der(ca_path, fp)?;
    let cert =
        SecCertificate::from_der(&der).map_err(|e| sec_err("read the CA certificate", e.code()))?;

    // SAFETY: `SecTrustSettingsRemoveTrustSettings` is a stable Security.framework
    // C function. We pass a valid `SecCertificateRef` borrowed from `cert` (alive
    // for the duration of the call; the callee does not retain it) and the
    // constant user-domain selector. Returns an `OSStatus` by value.
    let status = unsafe {
        SecTrustSettingsRemoveTrustSettings(cert.as_concrete_TypeRef(), kSecTrustSettingsDomainUser)
    };
    // errSecItemNotFound = nothing to remove (already untrusted) → success.
    if status != 0 && status != ERR_SEC_ITEM_NOT_FOUND {
        return Err(sec_err("remove the CA trust", status));
    }

    // We deliberately do NOT delete the cert from the keychain: removing the
    // trust setting is enough (verify-cert then reports not-trusted), and a
    // delete error must not mask a successful untrust.

    // Report whether the CA is still effectively trusted — a residual
    // admin/System-domain trust (set via terminal `sudo yerd elevate trust`)
    // can't be removed by an unprivileged GUI.
    yerd_platform::ActiveTrustStore::new()
        .is_trusted(ca_path, fp)
        .map_err(|e| GuiError::internal(format!("could not re-check trust state: {e}")))
}

/// Owner-check the CA file, read it once, and verify its first certificate's
/// SHA-256 against `fp`. Returns the verified DER — the exact bytes the caller
/// must import and trust (no second read between verify and use).
fn verified_der(ca_path: &Path, fp: &CaFingerprint) -> Result<Vec<u8>, GuiError> {
    require_user_owned(ca_path)?;
    let pem_text = std::fs::read(ca_path)
        .map_err(|e| GuiError::internal(format!("cannot read {}: {e}", ca_path.display())))?;
    let der = first_cert_der(&pem_text)
        .ok_or_else(|| GuiError::internal("the CA file contains no certificate"))?;
    let digest: [u8; 32] = Sha256::digest(&der).into();
    if &digest != fp.as_bytes() {
        return Err(GuiError::internal(
            "the CA file failed fingerprint verification and was not trusted",
        ));
    }
    Ok(der)
}

/// First `CERTIFICATE` block's DER body, or `None`.
fn first_cert_der(pem_bytes: &[u8]) -> Option<Vec<u8>> {
    pem::parse_many(pem_bytes)
        .ok()?
        .into_iter()
        .find(|b| b.tag() == "CERTIFICATE")
        .map(pem::Pem::into_contents)
}

/// Reject anything that isn't a regular file owned by us and not group/
/// world-writable. Uses `symlink_metadata` (lstat) and rejects symlinks — a
/// hardened variant of the CLI's `require_user_owned`.
fn require_user_owned(path: &Path) -> Result<(), GuiError> {
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::symlink_metadata(path)
        .map_err(|e| GuiError::internal(format!("cannot stat {}: {e}", path.display())))?;
    if md.file_type().is_symlink() {
        return Err(GuiError::internal(format!(
            "{} is a symlink; refusing to trust it",
            path.display()
        )));
    }
    if md.uid() != crate::elevate::current_uid() {
        return Err(GuiError::internal(format!(
            "{} is not owned by your user; refusing to trust it",
            path.display()
        )));
    }
    if md.mode() & 0o022 != 0 {
        return Err(GuiError::internal(format!(
            "{} is group/world-writable; refusing to trust it",
            path.display()
        )));
    }
    Ok(())
}

/// Map a Security-framework `OSStatus` to a clear, user-facing `GuiError`.
fn sec_err(action: &str, code: i32) -> GuiError {
    let msg = match code {
        ERR_AUTHORIZATION_CANCELED | ERR_USER_CANCELED => "Trust was cancelled.".to_owned(),
        ERR_SEC_AUTH_FAILED | ERR_SEC_INTERACTION_NOT_ALLOWED => {
            "Your login keychain is locked — unlock it and try again.".to_owned()
        }
        ERR_SEC_INTERNAL_COMPONENT => {
            "Couldn't show the trust prompt (no GUI session). Run `yerd elevate trust` in a terminal."
                .to_owned()
        }
        other => format!("Could not {action} (macOS error {other})."),
    };
    GuiError::internal(msg)
}
