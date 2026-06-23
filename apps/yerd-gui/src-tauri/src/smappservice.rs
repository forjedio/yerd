//! macOS: register the Yerd daemon as an `SMAppService` agent so the **Login
//! Items → Allow in the Background** entry is attributed to **Yerd.app** (app
//! name + icon) instead of the signing team (on an individual Apple Developer
//! account, the team name is the developer's legal name).
//!
//! This drives the launchd plist embedded at
//! `Yerd.app/Contents/Library/LaunchAgents/dev.yerd.daemon.plist` (shipped via
//! `bundle.macOS.files`; the `yerdd` it launches via `BundleProgram` is the
//! `externalBin` sidecar at `Contents/MacOS/yerdd`).
//!
//! `SMAppService` is macOS 13+; the crate floor is 13, so the class is always
//! present in a release bundle and the `ServiceManagement` framework is linked
//! normally (see `build.rs`). We still resolve the class dynamically and map a
//! missing class to a `GuiError` rather than asserting. Mirrors the hand-rolled
//! FFI style of `mac_trust.rs`: thin `unsafe` wrappers, every failure threaded
//! through [`GuiError`], no `unwrap`/`expect`/`panic`.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{NSError, NSString};

use crate::error::GuiError;

/// The agent plist filename, as embedded under `Contents/Library/LaunchAgents/`.
/// `+[SMAppService agentServiceWithPlistName:]` keys on this exact filename.
const PLIST_NAME: &str = "dev.yerd.daemon.plist";

// `SMAppServiceStatus` raw values (`NSInteger`).
pub(crate) const STATUS_NOT_REGISTERED: isize = 0;
pub(crate) const STATUS_ENABLED: isize = 1;
pub(crate) const STATUS_REQUIRES_APPROVAL: isize = 2;
pub(crate) const STATUS_NOT_FOUND: isize = 3;

/// Whether a status means the user-facing "run daemon at login" toggle is **on**.
/// `requiresApproval` counts as on: registration *succeeded*; the daemon just
/// won't launch until the user approves it in Login Items, so the switch must
/// not snap back to off. Pure (no FFI) — unit-tested.
pub(crate) fn status_means_registered(status: isize) -> bool {
    match status {
        STATUS_ENABLED | STATUS_REQUIRES_APPROVAL => true,
        STATUS_NOT_REGISTERED | STATUS_NOT_FOUND => false,
        // Unknown future value: treat as not-on (conservative).
        _ => false,
    }
}

/// Resolve the `SMAppService` class dynamically (never a static symbol
/// reference). Present on macOS 13+, i.e. always for us, but mapped to an error
/// rather than panicking if somehow absent.
fn smappservice_class() -> Result<&'static AnyClass, GuiError> {
    AnyClass::get(c"SMAppService")
        .ok_or_else(|| GuiError::internal("SMAppService is unavailable (requires macOS 13+)"))
}

/// `+[SMAppService agentServiceWithPlistName:@"dev.yerd.daemon.plist"]`.
/// Returns a non-null `SMAppService*` even when the plist is absent (its
/// `status` is then `notFound`); we still guard against a null return.
fn agent_service() -> Result<Retained<AnyObject>, GuiError> {
    let cls = smappservice_class()?;
    let name = NSString::from_str(PLIST_NAME);
    // SAFETY: `agentServiceWithPlistName:` is a class factory returning an
    // autoreleased `SMAppService*`; objc2 takes ownership per ARC conventions.
    // `name` is a valid `NSString` alive across the call. Typed as `Option` so a
    // (not expected) null is a recoverable error, never UB.
    let svc: Option<Retained<AnyObject>> =
        unsafe { msg_send![cls, agentServiceWithPlistName: &*name] };
    svc.ok_or_else(|| GuiError::internal("SMAppService returned no agent service"))
}

/// Register (enable) the agent. On success the agent is registered as a login
/// item; with `RunAtLoad` it also starts now. **Success includes the
/// `requiresApproval` case** — `registerAndReturnError:` returns `true` and the
/// user must enable it in Login Items; the caller reads [`status`] to decide
/// whether to nudge the user. Idempotent: registering an already-registered
/// service succeeds.
pub(crate) fn register() -> Result<(), GuiError> {
    let svc = agent_service()?;
    // SAFETY: `-registerAndReturnError:` returns `BOOL` with a trailing
    // `NSError**`; the `_` marker activates objc2's BOOL→Result handling.
    let res: Result<(), Retained<NSError>> = unsafe { msg_send![&*svc, registerAndReturnError: _] };
    res.map_err(|e| ns_err("register the Yerd background daemon", &e))
}

/// Unregister (disable) the agent: removes the login item / "Yerd" entry and
/// unloads the job. `errSec`-style "not registered" is reported by the OS as
/// success here, so a redundant unregister is harmless.
pub(crate) fn unregister() -> Result<(), GuiError> {
    let svc = agent_service()?;
    // SAFETY: as `register`, for `-unregisterAndReturnError:`.
    let res: Result<(), Retained<NSError>> =
        unsafe { msg_send![&*svc, unregisterAndReturnError: _] };
    res.map_err(|e| ns_err("remove the Yerd background daemon registration", &e))
}

/// The agent's `SMAppServiceStatus` (one of the `STATUS_*` constants). Read-only
/// — safe to call at startup to populate the UI without mutating anything.
pub(crate) fn status() -> Result<isize, GuiError> {
    let svc = agent_service()?;
    // SAFETY: `-status` is a property getter returning `NSInteger`.
    let s: isize = unsafe { msg_send![&*svc, status] };
    Ok(s)
}

/// Deep-link to **System Settings → General → Login Items** (used when status is
/// `requiresApproval`). Best-effort; a failure to resolve the class is ignored.
pub(crate) fn open_login_items_settings() {
    if let Ok(cls) = smappservice_class() {
        // SAFETY: `+openSystemSettingsLoginItems` is a class method, no args,
        // returns void.
        let _: () = unsafe { msg_send![cls, openSystemSettingsLoginItems] };
    }
}

/// Map an `NSError` from a register/unregister failure to a user-facing
/// `GuiError`. `localizedDescription` is the human string; the integer `code`
/// aids diagnosis (e.g. translocation / not-in-/Applications failures).
fn ns_err(action: &str, err: &NSError) -> GuiError {
    // SAFETY: `code`/`localizedDescription` are standard `NSError` getters; the
    // returned `NSString` is autoreleased and only read here.
    let code: isize = unsafe { msg_send![err, code] };
    let desc: Retained<NSString> = unsafe { msg_send![err, localizedDescription] };
    GuiError::internal(format!("Could not {action}: {desc} (macOS error {code})."))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn status_on_off_classification() {
        assert!(status_means_registered(STATUS_ENABLED));
        assert!(status_means_registered(STATUS_REQUIRES_APPROVAL));
        assert!(!status_means_registered(STATUS_NOT_REGISTERED));
        assert!(!status_means_registered(STATUS_NOT_FOUND));
    }

    /// Runtime FFI smoke test: `agentServiceWithPlistName:` + `status` must
    /// round-trip on a real ServiceManagement runtime (macOS 13+) without
    /// mutating anything — `status` is a read-only query, and for a plist that
    /// isn't part of a registered bundle it returns `notFound`. This catches
    /// `msg_send!` signature mistakes that compile but are UB at call time.
    /// `#[ignore]` so plain `cargo test` (incl. CI) never issues an SMAppService
    /// query; run locally with `cargo test -p yerd-gui -- --ignored`.
    #[test]
    #[ignore = "issues a read-only SMAppService query; run manually on macOS 13+"]
    fn status_ffi_roundtrips_readonly() {
        let s = status().expect("status() should query without error");
        assert!(
            (STATUS_NOT_REGISTERED..=STATUS_NOT_FOUND).contains(&s),
            "unexpected SMAppServiceStatus {s}"
        );
    }
}
