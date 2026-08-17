//! Window zoom for Yerd's borderless macOS windows.
//!
//! Every Yerd window is `decorations: false`, so its `NSWindow` carries no
//! `Titled` style mask and tao refuses the native `zoom:` path. Its fallback,
//! `setFrame:display:NO animate:YES`, is wrong twice over: it suppresses redraw,
//! and it animates *synchronously*, blocking the main thread in a nested run
//! loop for the whole animation. The webview renders out of process, so it
//! cannot deliver a repaint until that returns - and because the window is
//! `transparent: true`, the not-yet-painted area a growing window exposes is
//! see-through. The result reads as the old-size window sliding into the corner
//! and snapping to size, while shrinking (which only ever clips) looks fine.
//!
//! Animating through the `animator` proxy instead keeps the run loop turning,
//! so the webview repaints as the frame moves and the window grows and shrinks
//! like any other.
//!
//! State stays consistent with tao: for a borderless window its `is_maximized`
//! compares the window frame against the screen's visible frame, which is the
//! frame this module zooms to.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use objc2_app_kit::{NSAnimatablePropertyContainer, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use tauri::{Manager, WebviewWindow};

/// How long a scheduled frame animation is assumed to still be running. AppKit's
/// implicit animation duration is 0.25s; the margin only has to outlast that.
const ANIMATION_SETTLE: Duration = Duration::from_millis(400);

/// Per-window zoom bookkeeping, keyed by window label. `NSRect` is plain `f64`
/// geometry, so it crosses the mutex freely.
static ZOOM_STATES: OnceLock<Mutex<HashMap<String, ZoomState>>> = OnceLock::new();

/// What this module has asked of one window.
#[derive(Clone, Copy, Default)]
struct ZoomState {
    /// The frame to un-zoom to, saved on the way into a zoom.
    restore: Option<NSRect>,
    /// The frame the last animation was asked to produce, and when. While that
    /// animation runs, `NSWindow::frame` interpolates towards it, so a toggle
    /// arriving mid-flight has to reason about the target instead of the live
    /// frame - otherwise it reads a half-grown window as "not zoomed" and
    /// overwrites `restore` with that intermediate rect.
    target: Option<(NSRect, Instant)>,
}

impl ZoomState {
    /// The frame the window is settling on, if an animation could still be in
    /// flight. `None` once it has settled, when the live frame is authoritative
    /// again (the user may have moved or resized the window since).
    fn in_flight_target(&self, now: Instant) -> Option<NSRect> {
        self.target
            .filter(|(_, since)| now.duration_since(*since) < ANIMATION_SETTLE)
            .map(|(target, _)| target)
    }
}

/// Zoom `window` to its screen's visible frame, or back to the frame it had
/// before the last zoom. Runs on the main thread, where AppKit requires it, and
/// returns as soon as the animation is scheduled rather than driving it.
pub(crate) fn toggle(window: &WebviewWindow) {
    let owned = window.clone();
    let _ = window.run_on_main_thread(move || toggle_on_main(&owned));
}

fn toggle_on_main(window: &WebviewWindow) {
    let Some(ns_window) = ns_window(window) else {
        return;
    };
    if ns_window
        .styleMask()
        .contains(NSWindowStyleMask::FullScreen)
    {
        return;
    }
    let Some(screen) = ns_window.screen() else {
        return;
    };

    let mut state = load_state(window.label());
    let target = next_target(
        &mut state,
        ns_window.frame(),
        screen.visibleFrame(),
        configured_size(window),
        Instant::now(),
    );
    store_state(window.label(), state);

    let Some(target) = target else {
        return;
    };
    ns_window.animator().setFrame_display(target, true);
}

/// Decide the frame a toggle should animate to, advancing `state`. Returns
/// `None` when there is nothing to un-zoom to.
///
/// Pure: the caller supplies the live frame, the screen's visible frame and the
/// window's configured size, so the whole decision is unit-testable without
/// AppKit.
fn next_target(
    state: &mut ZoomState,
    frame: NSRect,
    visible: NSRect,
    configured: Option<NSSize>,
    now: Instant,
) -> Option<NSRect> {
    let current = state.in_flight_target(now).unwrap_or(frame);
    let target = if is_zoomed(current, visible) {
        match state.restore.take() {
            Some(restored) => restored,
            None => centered(configured?, visible),
        }
    } else {
        state.restore = Some(current);
        visible
    };
    state.target = Some((target, now));
    Some(target)
}

/// Borrow the window's live `NSWindow`.
fn ns_window(window: &WebviewWindow) -> Option<&NSWindow> {
    let ptr = window.ns_window().ok()?;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: ns_window() returns the window's live NSWindow pointer, and every
    // caller is already on the main thread (toggle() dispatches there), where
    // AppKit window access must happen.
    Some(unsafe { &*ptr.cast::<NSWindow>() })
}

/// Whether the window already fills its screen's visible frame. Mirrors tao's
/// borderless `is_maximized` check - including its 1pt tolerance - so
/// `Window::is_maximized` and this module never disagree.
fn is_zoomed(frame: NSRect, visible: NSRect) -> bool {
    (frame.size.width - visible.size.width).abs() < 1.0
        && (frame.size.height - visible.size.height).abs() < 1.0
}

fn load_state(label: &str) -> ZoomState {
    ZOOM_STATES
        .get_or_init(Mutex::default)
        .lock()
        .ok()
        .and_then(|states| states.get(label).copied())
        .unwrap_or_default()
}

fn store_state(label: &str, state: ZoomState) {
    if let Ok(mut states) = ZOOM_STATES.get_or_init(Mutex::default).lock() {
        states.insert(label.to_owned(), state);
    }
}

/// The window's declared size from `tauri.conf.json`, in points (AppKit
/// geometry and Tauri's logical sizes share a unit). Un-zooming falls back to
/// it, centred, for a window zoomed by something other than this module - macOS
/// window tiling fills the visible frame exactly, which `is_zoomed` cannot tell
/// apart from our own zoom.
fn configured_size(window: &WebviewWindow) -> Option<NSSize> {
    window
        .config()
        .app
        .windows
        .iter()
        .find(|cfg| cfg.label == window.label())
        .map(|cfg| NSSize::new(cfg.width, cfg.height))
}

fn centered(size: NSSize, visible: NSRect) -> NSRect {
    let origin = NSPoint::new(
        visible.origin.x + (visible.size.width - size.width) / 2.0,
        visible.origin.y + (visible.size.height - size.height) / 2.0,
    );
    NSRect::new(origin, size)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::{centered, is_zoomed, next_target, ZoomState, ANIMATION_SETTLE};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use std::time::{Duration, Instant};

    const VISIBLE: NSRect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1440.0, 875.0));
    const CONFIGURED: NSSize = NSSize::new(1040.0, 860.0);

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }

    fn same(a: NSRect, b: NSRect) -> bool {
        a.origin.x == b.origin.x
            && a.origin.y == b.origin.y
            && a.size.width == b.size.width
            && a.size.height == b.size.height
    }

    /// One toggle at `now`, with `frame` as the window's live frame.
    fn toggle(state: &mut ZoomState, frame: NSRect, now: Instant) -> Option<NSRect> {
        next_target(state, frame, VISIBLE, Some(CONFIGURED), now)
    }

    #[test]
    fn is_zoomed_matches_the_visible_frame_within_a_point() {
        let visible = rect(0.0, 0.0, 1440.0, 875.0);
        for (frame, expected) in [
            (visible, true),
            (rect(0.0, 0.0, 1439.5, 874.5), true),
            (rect(200.0, 100.0, 1440.0, 875.0), true),
            (rect(0.0, 0.0, 1438.0, 875.0), false),
            (rect(0.0, 0.0, 1440.0, 800.0), false),
            (rect(0.0, 0.0, 1040.0, 860.0), false),
        ] {
            assert_eq!(is_zoomed(frame, visible), expected);
        }
    }

    #[test]
    fn centered_places_the_window_in_the_middle_of_the_visible_frame() {
        let visible = rect(100.0, 50.0, 1440.0, 900.0);
        let frame = centered(NSSize::new(1040.0, 860.0), visible);
        assert_eq!(frame.origin.x, 300.0);
        assert_eq!(frame.origin.y, 70.0);
        assert_eq!(frame.size.width, 1040.0);
        assert_eq!(frame.size.height, 860.0);
    }

    #[test]
    fn a_settled_toggle_round_trips_through_the_original_frame() {
        let mut state = ZoomState::default();
        let original = rect(80.0, 60.0, 1040.0, 860.0);
        let now = Instant::now();

        let zoomed = toggle(&mut state, original, now).expect("zooms");
        assert!(same(zoomed, VISIBLE));

        let settled = now + ANIMATION_SETTLE;
        let restored = toggle(&mut state, VISIBLE, settled).expect("restores");
        assert!(same(restored, original));
    }

    #[test]
    fn a_toggle_mid_animation_reverses_it_without_losing_the_original_frame() {
        let mut state = ZoomState::default();
        let original = rect(80.0, 60.0, 1040.0, 860.0);
        let now = Instant::now();

        toggle(&mut state, original, now);

        let mid_zoom = rect(40.0, 30.0, 1240.0, 868.0);
        let reversed = toggle(&mut state, mid_zoom, now + Duration::from_millis(100))
            .expect("reverses the zoom");
        assert!(same(reversed, original));

        let mid_restore = rect(60.0, 45.0, 1140.0, 864.0);
        let rezoomed =
            toggle(&mut state, mid_restore, now + Duration::from_millis(200)).expect("re-zooms");
        assert!(same(rezoomed, VISIBLE));

        let settled = now + Duration::from_millis(200) + ANIMATION_SETTLE;
        let restored = toggle(&mut state, VISIBLE, settled).expect("restores");
        assert!(same(restored, original));
    }

    #[test]
    fn a_window_zoomed_by_the_system_restores_to_its_configured_size() {
        let mut state = ZoomState::default();

        let restored = toggle(&mut state, VISIBLE, Instant::now()).expect("restores");
        assert!(same(restored, centered(CONFIGURED, VISIBLE)));
    }

    #[test]
    fn a_manual_resize_after_the_animation_settles_is_taken_at_face_value() {
        let mut state = ZoomState::default();
        let original = rect(80.0, 60.0, 1040.0, 860.0);
        let now = Instant::now();

        toggle(&mut state, original, now);

        let dragged = rect(10.0, 20.0, 700.0, 500.0);
        let settled = now + ANIMATION_SETTLE;
        let zoomed = toggle(&mut state, dragged, settled).expect("zooms");
        assert!(same(zoomed, VISIBLE));

        let restored = toggle(&mut state, VISIBLE, settled + ANIMATION_SETTLE).expect("restores");
        assert!(same(restored, dragged));
    }

    #[test]
    fn an_unconfigured_window_zoomed_by_the_system_is_left_alone() {
        let mut state = ZoomState::default();

        assert!(next_target(&mut state, VISIBLE, VISIBLE, None, Instant::now()).is_none());
    }
}
