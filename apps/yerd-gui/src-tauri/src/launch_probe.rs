//! macOS: detect whether this process was launched **at login** (an
//! `SMAppService.mainApp` / login-item launch) versus a **manual** open, so the
//! GUI can honor the "start minimized to the tray" preference.
//!
//! `SMAppService.mainApp` cannot inject a launch argument (the
//! `--autostarted` marker the old plugin-LaunchAgent path relied on), so instead
//! we read AppKit's `NSApplicationLaunchIsDefaultLaunchKey` from the
//! `applicationDidFinishLaunching` notification: it is `false` for a login-item
//! launch (and for state-restoration), `true` for a Finder/Dock/`open` launch.
//!
//! INVARIANT — the safe-failure guarantee below holds ONLY while Yerd registers
//! no `CFBundleURLSchemes` and no `CFBundleDocumentTypes`. Those also launch with
//! `LaunchIsDefault == false` and would be misread as a login launch (window
//! wrongly hidden). Revisit this probe before adding any URL scheme / doc type.
//!
//! Mirrors the hand-rolled FFI style of `smappservice.rs`/`mac_trust.rs`: thin
//! `unsafe` wrappers, no typed-binding feature creep — `userInfo`/`boolValue` are
//! read via raw `msg_send!`.

use std::ptr::NonNull;
use std::sync::atomic::{AtomicU8, Ordering};

use block2::RcBlock;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_app_kit::{
    NSApplicationDidFinishLaunchingNotification, NSApplicationLaunchIsDefaultLaunchKey,
};

const UNKNOWN: u8 = 0;
const LOGIN: u8 = 1;
const MANUAL: u8 = 2;

/// Set once, from the `applicationDidFinishLaunching` observer block. Read later
/// (on a deferred main-runloop turn) by [`is_login_launch`].
static LAUNCH_KIND: AtomicU8 = AtomicU8::new(UNKNOWN);

/// Pure mapping from AppKit's `NSApplicationLaunchIsDefaultLaunchKey` to a launch
/// kind: a "default" launch (Finder/Dock/`open`) is `MANUAL`; everything else
/// (login item, state-restoration) is `LOGIN`. Split out so it's unit-testable
/// without AppKit firing a notification.
const fn kind_for(is_default: bool) -> u8 {
    if is_default {
        MANUAL
    } else {
        LOGIN
    }
}

/// Register the launch-type observer. Call **before** `tauri::Builder::run()` so
/// the observer exists when AppKit posts `applicationDidFinishLaunching`. Reading
/// `LAUNCH_KIND` must be deferred one runloop turn after `setup` (the
/// notification dispatch may not be complete when `setup` runs) — see
/// `show_initial_window`. Best-effort: a failure to resolve the class leaves the
/// state `UNKNOWN`, which [`is_login_launch`] treats as a manual open (safe).
pub(crate) fn install_launch_probe() {
    let Some(cls) = AnyClass::get(c"NSNotificationCenter") else {
        return;
    };
    // SAFETY: `+defaultCenter` is a class singleton getter returning a non-null
    // `NSNotificationCenter*`.
    let center: Retained<AnyObject> = unsafe { msg_send![cls, defaultCenter] };

    let block = RcBlock::new(move |note: NonNull<AnyObject>| {
        // SAFETY: AppKit hands the block a valid, non-null `NSNotification*`.
        let note: &AnyObject = unsafe { note.as_ref() };
        // SAFETY: `-userInfo` is a getter returning `NSDictionary*` or nil.
        let user_info: *mut AnyObject = unsafe { msg_send![note, userInfo] };
        if user_info.is_null() {
            return; // no userInfo → leave UNKNOWN (treated as manual)
        }
        // SAFETY: extern string constant from AppKit; a valid `&NSString`.
        let key = unsafe { NSApplicationLaunchIsDefaultLaunchKey };
        // SAFETY: `-objectForKey:` returns the `NSNumber*` value or nil.
        let val: *mut AnyObject = unsafe { msg_send![user_info, objectForKey: key] };
        if val.is_null() {
            return; // key absent → leave UNKNOWN
        }
        // SAFETY: the value is an `NSNumber`; `-boolValue` returns its `BOOL`.
        let is_default: bool = unsafe { msg_send![val, boolValue] };
        LAUNCH_KIND.store(kind_for(is_default), Ordering::Relaxed);
    });

    // SAFETY: standard `-addObserverForName:object:queue:usingBlock:`. `object`
    // and `queue` are nil (observe every post, deliver synchronously on the
    // posting thread). The returned observer token must outlive the observation,
    // so we leak it for the process lifetime (there is nothing to unregister).
    let name = unsafe { NSApplicationDidFinishLaunchingNotification };
    let token: Option<Retained<AnyObject>> = unsafe {
        msg_send![
            &center,
            addObserverForName: name,
            object: None::<&AnyObject>,
            queue: None::<&AnyObject>,
            usingBlock: &*block,
        ]
    };
    if let Some(token) = token {
        std::mem::forget(token);
    }
}

/// True only when we **positively** detected a login/system launch. `UNKNOWN`
/// maps to `false` — the safe direction: a manual Finder/Dock/`open` launch
/// always sets `LaunchIsDefault = true`, so the window is never wrongly hidden on
/// a real user open; at worst a login launch is missed and the window shows.
pub(crate) fn is_login_launch() -> bool {
    LAUNCH_KIND.load(Ordering::Relaxed) == LOGIN
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Pure mapping coverage (runs in normal CI, unlike the `#[ignore]` FFI test):
    /// a "default" launch is manual; a non-default launch (login item / restore)
    /// is a login launch; the unset state is neither.
    #[test]
    fn kind_for_maps_default_and_login() {
        assert_eq!(kind_for(true), MANUAL);
        assert_eq!(kind_for(false), LOGIN);
        assert_ne!(LOGIN, UNKNOWN);
        assert_ne!(MANUAL, UNKNOWN);
    }

    /// Runtime smoke test: registering the observer exercises the
    /// `addObserverForName:…:usingBlock:` `msg_send!` and the block construction —
    /// the spot a signature mistake would be UB at call time. Safe without a full
    /// NSApplication (NSNotificationCenter always exists); the observer simply
    /// never fires here. `#[ignore]` so plain `cargo test` never registers an
    /// observer; run with `cargo test -p yerd-gui -- --ignored`.
    #[test]
    #[ignore = "registers a real NSNotificationCenter observer; run manually on macOS"]
    fn install_probe_roundtrips() {
        install_launch_probe();
        // Before any launch notification fires in-test, the state is UNKNOWN.
        assert!(!is_login_launch());
    }
}
