//! Frameless tray popup panel (site autocomplete + per-site actions).
//!
//! Declared statically in `tauri.conf.json` (label `tray-panel`, hidden). Shown
//! under the tray icon on the active monitor; hidden on blur or Escape. Reused
//! as the Linux no-tray fallback surface.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::{Emitter, Manager, PhysicalPosition, Position, Rect, Size, WebviewWindow};

use crate::error::GuiError;

const PANEL_ID: &str = "tray-panel";
const TRAY_ID: &str = "yerd-tray";
/// Logical size mirrors `tauri.conf.json`.
const PANEL_SIZE: (f64, f64) = (380.0, 560.0);
/// Screen inset when clamping the panel on the active monitor.
const MARGIN_LOGICAL: f64 = 12.0;
/// Typical macOS menu-bar height (logical pt) when tray bounds are unavailable.
const MENU_BAR_LOGICAL: f64 = 28.0;
/// ~22pt macOS status-item glyph width (logical pt) when bounds are unavailable.
const ICON_WIDTH_LOGICAL: f64 = 22.0;

/// Set when AppIndicator / tray registration failed and we should keep an
/// edge-docked panel available as the primary control surface.
static TRAY_FALLBACK: AtomicBool = AtomicBool::new(false);
/// Last tray-icon bounds from a click (`left_x`, `bottom_y`) in physical px.
static TRAY_ICON_ANCHOR: Mutex<Option<(i32, i32)>> = Mutex::new(None);

pub(crate) fn set_tray_fallback(on: bool) {
    TRAY_FALLBACK.store(on, Ordering::Release);
}

pub(crate) fn tray_fallback() -> bool {
    TRAY_FALLBACK.load(Ordering::Acquire)
}

/// Remember the tray icon bounds from a [`tauri::tray::TrayIconEvent::Click`].
pub(crate) fn note_tray_icon_rect(rect: &Rect) {
    let (left_x, bottom_y) = rect_left_bottom(rect);
    if let Ok(mut anchor) = TRAY_ICON_ANCHOR.lock() {
        *anchor = Some((left_x, bottom_y));
    }
}

/// Leading/bottom edges of a tray [`Rect`] in physical pixels.
fn rect_left_bottom(rect: &Rect) -> (i32, i32) {
    let (x, y, _w, h) = rect_physical_edges(rect);
    (x, y + h)
}

fn rect_physical_edges(rect: &Rect) -> (i32, i32, i32, i32) {
    let (x, y) = match rect.position {
        Position::Physical(p) => (p.x, p.y),
        Position::Logical(p) => (p.x as i32, p.y as i32),
    };
    let (w, h) = match rect.size {
        Size::Physical(s) => (s.width as i32, s.height as i32),
        Size::Logical(s) => (s.width as i32, s.height as i32),
    };
    (x, y, w, h)
}

/// Toggle the tray panel: show+focus if hidden, hide if visible.
pub(crate) fn toggle_tray_panel(app: &tauri::AppHandle) -> Result<(), GuiError> {
    let win = panel_window(app)?;
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        return Ok(());
    }
    show_tray_panel(app)
}

/// Show and focus the tray panel, positioned under the menu-bar tray icon.
pub(crate) fn show_tray_panel(app: &tauri::AppHandle) -> Result<(), GuiError> {
    let win = panel_window(app)?;
    // Apply always-on-top at show time (setting it in tauri.conf at create time
    // crashes on macOS via NSWindow collectionBehavior while still hidden).
    let _ = win.set_always_on_top(true);
    position_panel(&win, app);
    let _ = win.show();
    let _ = win.set_focus();
    let _ = app.emit("tray-panel-opened", ());
    Ok(())
}

pub(crate) fn hide_tray_panel(app: &tauri::AppHandle) -> Result<(), GuiError> {
    if let Some(win) = app.get_webview_window(PANEL_ID) {
        let _ = win.set_always_on_top(false);
        let _ = win.hide();
    }
    Ok(())
}

fn panel_window(app: &tauri::AppHandle) -> Result<WebviewWindow, GuiError> {
    app.get_webview_window(PANEL_ID)
        .ok_or_else(|| GuiError::internal("tray-panel window is not configured"))
}

fn tray_icon_anchor(app: &tauri::AppHandle) -> Option<(i32, i32)> {
    if let Ok(guard) = TRAY_ICON_ANCHOR.lock() {
        if let Some(anchor) = *guard {
            return Some(anchor);
        }
    }
    let tray = app.tray_by_id(TRAY_ID)?;
    let rect = tray.rect().ok()??;
    Some(rect_left_bottom(&rect))
}

/// Place the panel under the tray icon (bounds from the click or `TrayIcon::rect`),
/// left-aligned with the icon when it fits on-screen, otherwise shifted left.
#[allow(clippy::cast_possible_truncation)]
fn position_panel(win: &WebviewWindow, app: &tauri::AppHandle) {
    let (cfg_w, cfg_h) = PANEL_SIZE;
    let anchor = tray_icon_anchor(app);
    let cursor = win.cursor_position().ok();
    let monitor = anchor
        .or_else(|| cursor.map(|c| (c.x as i32, c.y as i32)))
        .and_then(|(x, y)| win.monitor_from_point(x as f64, y as f64).ok().flatten())
        .or_else(|| win.current_monitor().ok().flatten())
        .or_else(|| win.primary_monitor().ok().flatten());

    let Some(monitor) = monitor else {
        return;
    };
    let scale = monitor.scale_factor();
    let pos = monitor.position();
    let size = monitor.size();
    let layout = compute_panel_layout(
        tray_fallback(),
        anchor.map(|a| a.0),
        anchor.map(|a| a.1),
        cursor.map(|c| c.x as i32),
        pos.x,
        pos.y,
        size.width as i32,
        size.height as i32,
        (cfg_w * scale) as i32,
        (cfg_h * scale) as i32,
        (MARGIN_LOGICAL * scale) as i32,
        (MENU_BAR_LOGICAL * scale) as i32,
        (ICON_WIDTH_LOGICAL * scale) as i32,
    );
    let _ = win.set_position(PhysicalPosition::new(layout.x, layout.y));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PanelLayout {
    x: i32,
    y: i32,
}

/// Pure layout: left-align the panel with the tray icon when it fits on-screen;
/// shift left only when the panel would overflow the monitor's right edge.
#[allow(clippy::too_many_arguments)]
fn compute_panel_layout(
    fallback_mode: bool,
    icon_left_x: Option<i32>,
    icon_bottom_y: Option<i32>,
    cursor_x: Option<i32>,
    mon_x: i32,
    mon_y: i32,
    mon_w: i32,
    mon_h: i32,
    panel_w: i32,
    panel_h: i32,
    margin: i32,
    menu_bar_h: i32,
    icon_width: i32,
) -> PanelLayout {
    let mon_right = mon_x + mon_w;
    let mon_bottom = mon_y + mon_h;
    let min_x = mon_x + margin;
    let max_x = mon_right - panel_w - margin;

    let x = if fallback_mode {
        mon_right - panel_w - margin
    } else {
        let icon_left =
            icon_left_x.unwrap_or_else(|| cursor_x.map(|cx| cx - icon_width / 2).unwrap_or(max_x));
        let mut x = icon_left;
        if x + panel_w > mon_right - margin {
            x = max_x;
        }
        if min_x <= max_x {
            x = x.clamp(min_x, max_x);
        } else {
            x = min_x;
        }
        x
    };

    let y = if fallback_mode {
        mon_y + margin
    } else {
        icon_bottom_y.unwrap_or(mon_y + menu_bar_h)
    };
    let max_y = mon_bottom - panel_h - margin;
    let y = y.min(max_y).max(mon_y + margin);

    PanelLayout { x, y }
}

/// Toggle the tray panel window (show if hidden, hide if visible).
#[tauri::command]
pub fn toggle_tray_panel_cmd(app: tauri::AppHandle) -> Result<(), GuiError> {
    toggle_tray_panel(&app)
}

/// Hide the tray panel window without quitting the app.
#[tauri::command]
pub fn hide_tray_panel_cmd(app: tauri::AppHandle) -> Result<(), GuiError> {
    hide_tray_panel(&app)
}

/// Whether the GUI is using the tray-panel fallback (native tray unavailable).
#[tauri::command]
pub fn tray_fallback_active() -> bool {
    tray_fallback()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: (i32, i32, i32, i32) = (0, 0, 1920, 1080);
    const PANEL: (i32, i32) = (380, 560);
    const MARGIN: i32 = 12;
    const MENU_BAR: i32 = 28;
    const ICON_WIDTH: i32 = 22;

    fn layout_left(icon_left: i32) -> PanelLayout {
        let (mx, my, mw, mh) = MON;
        let (pw, ph) = PANEL;
        compute_panel_layout(
            false,
            Some(icon_left),
            Some(MENU_BAR),
            None,
            mx,
            my,
            mw,
            mh,
            pw,
            ph,
            MARGIN,
            MENU_BAR,
            ICON_WIDTH,
        )
    }

    #[test]
    fn left_aligns_with_icon_when_room_on_screen() {
        let l = layout_left(1500);
        assert_eq!(l.x, 1500);
        assert_eq!(l.y, MENU_BAR);
    }

    #[test]
    fn shifts_left_when_icon_near_screen_edge() {
        // Icon at x=1889; panel 380 wide would end past 1908 margin.
        let l = layout_left(1889);
        assert_eq!(l.x, MON.2 - PANEL.0 - MARGIN);
    }

    #[test]
    fn clamps_when_panel_would_overflow_left() {
        let (mx, my, mw, mh) = MON;
        let (pw, ph) = PANEL;
        let l = compute_panel_layout(
            false,
            Some(4),
            Some(MENU_BAR),
            None,
            mx,
            my,
            mw,
            mh,
            pw,
            ph,
            MARGIN,
            MENU_BAR,
            ICON_WIDTH,
        );
        assert_eq!(l.x, MARGIN);
    }

    #[test]
    fn defaults_to_top_right_without_bounds() {
        let (mx, my, mw, mh) = MON;
        let (pw, ph) = PANEL;
        let l = compute_panel_layout(
            false, None, None, None, mx, my, mw, mh, pw, ph, MARGIN, MENU_BAR, ICON_WIDTH,
        );
        assert_eq!(l.x, mw - pw - MARGIN);
        assert_eq!(l.y, MENU_BAR);
    }

    #[test]
    fn fallback_pins_top_right() {
        let (mx, my, mw, mh) = MON;
        let (pw, ph) = PANEL;
        let l = compute_panel_layout(
            true,
            Some(500),
            Some(28),
            Some(500),
            mx,
            my,
            mw,
            mh,
            pw,
            ph,
            MARGIN,
            MENU_BAR,
            ICON_WIDTH,
        );
        assert_eq!(l.x, mw - pw - MARGIN);
        assert_eq!(l.y, MARGIN);
    }
}
