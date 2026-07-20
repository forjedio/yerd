//! The system-tray icon and its **dynamic** dropdown menu.
//!
//! Unlike the rest of the GUI, the tray must stay correct while the main window
//! is closed to tray - exactly when the frontend's daemon poller pauses (it
//! short-circuits on `document.visibilityState === "hidden"`). So the tray owns a
//! small **Rust-side poll** over the same `yerd-ipc` socket the commands use,
//! rebuilding the menu only when a diffed snapshot of daemon state actually
//! changes. This stays a thin client: it only calls `yerd-ipc`, never daemon
//! logic.
//!
//! Concurrency: a tray-initiated daemon lifecycle action (start/stop/restart)
//! owns the menu for the duration of the action via the `TRANSITION` flag, and
//! `MENU_LOCK` makes the poller's "is a transition active? then apply" decision
//! atomic with respect to the action clearing the flag - so a poller tick that
//! began its fetch before the action can't overwrite the action's transient menu.
//! `MENU_LOCK` is only ever taken from spawned worker tasks, never the main
//! thread (a worker blocks on the main thread while `set_menu` marshals, so a
//! main-thread lock would cycle-deadlock). Callers read `dark_menu_bar()`
//! themselves *before* taking the lock (on Linux it's a bounded D-Bus round
//! trip) and pass the result in, so the lock only ever wraps the menu-building
//! and `set_menu`/`set_icon` marshalling it was meant to serialize.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tauri::menu::{IconMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Wry};

use yerd_core::PhpVersion;
use yerd_ipc::{Request, Response, ServiceRunState, StatusReport};

use crate::ipc::{exchange, exchange_timeout};
use crate::tray_health::{derive_health, tray_dropdown_service_rows, ServiceRow, TrayHealth};

const TRAY_ID: &str = "yerd-tray";
/// Background poll cadence. Modest because the tray is rarely open; diff-gating
/// means most ticks don't rebuild anything.
const POLL_INTERVAL: Duration = Duration::from_secs(6);
/// Bound for each tray status probe so a wedged daemon can't stall the poller.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounded wait for a daemon lifecycle action to settle: `SETTLE_STEPS` steps of
/// a `SETTLE_STEP` sleep plus a short `SETTLE_PROBE_TIMEOUT` status probe, so the
/// whole transition stays near 10s even if every probe times out (rather than the
/// ~110s a 5s `PROBE_TIMEOUT` per step would give against a half-wedged socket).
const SETTLE_STEPS: u32 = 20;
const SETTLE_STEP: Duration = Duration::from_millis(500);
/// Per-probe timeout inside the settle loops - short so an unresponsive daemon
/// can't blow the settle bound far past `SETTLE_STEPS × SETTLE_STEP`.
const SETTLE_PROBE_TIMEOUT: Duration = Duration::from_millis(400);

/// Bundled menu-item icons (lucide, rasterised to 36px black PNGs, the glyph
/// padded to ~75% of the canvas so muda's fixed 18pt menu icon isn't oversized;
/// recoloured to white for dark mode at runtime by `menu_icon`, since muda
/// doesn't treat menu icons as templates).
mod icons {
    pub const OPEN: &[u8] = include_bytes!("../icons/menu/app-window.png");
    pub const UPDATE: &[u8] = include_bytes!("../icons/menu/download.png");
    pub const UPDATE_PHP: &[u8] = include_bytes!("../icons/menu/arrow-down-to-line.png");
    pub const NEW_SITE: &[u8] = include_bytes!("../icons/menu/rocket.png");
    pub const LINK: &[u8] = include_bytes!("../icons/menu/link.png");
    pub const PARK: &[u8] = include_bytes!("../icons/menu/folder-plus.png");
    pub const RESTART: &[u8] = include_bytes!("../icons/menu/rotate-cw.png");
    pub const STOP: &[u8] = include_bytes!("../icons/menu/square.png");
    pub const START: &[u8] = include_bytes!("../icons/menu/play.png");
    pub const CHECK_UPDATES: &[u8] = include_bytes!("../icons/menu/refresh-cw.png");
    pub const MAIL: &[u8] = include_bytes!("../icons/menu/mail.png");
    pub const DUMPS: &[u8] = include_bytes!("../icons/menu/clipboard-list.png");
    pub const SITES: &[u8] = include_bytes!("../icons/menu/layout-grid.png");
    pub const SERVICES: &[u8] = include_bytes!("../icons/menu/database.png");
    pub const ABOUT: &[u8] = include_bytes!("../icons/menu/info.png");
    pub const DOCTOR: &[u8] = include_bytes!("../icons/menu/stethoscope.png");
    pub const QUIT: &[u8] = include_bytes!("../icons/menu/power.png");
}

/// The navigable pages the tray used to link; retained for possible reuse.
#[allow(dead_code)]
const NAV_ITEMS: &[(&str, &str, &[u8])] = &[
    ("nav:/sites", "Sites", icons::SITES),
    ("nav:/services", "Services", icons::SERVICES),
    ("nav:/about", "About", icons::ABOUT),
];

/// The menu-bar glyph: rounded squircle with Y cut out (template source:
/// `icons/tray-mac.svg`), rasterised at build time (22pt @4x = 88px) for crisp
/// Retina menu bars. Used for the macOS template icon and for the
/// [`TrayIconVariant::LightY`]/`DarkY` overrides on every OS (see `main.rs` for
/// the macOS template rationale).
const TRAY_ICON_MAC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tray-mac.png"));

/// Set while a tray-initiated daemon lifecycle action owns the menu.
static TRANSITION: AtomicBool = AtomicBool::new(false);
/// Serializes the poller's `check TRANSITION + set_menu` with an action's clear.
/// Held only around a synchronous `set_menu`, never across an `.await`, and never
/// from the main thread.
static MENU_LOCK: Mutex<()> = Mutex::new(());

/// Take `MENU_LOCK`, recovering a poisoned lock rather than panicking (a poisoned
/// menu lock guards no critical invariant; `.unwrap()` would trip the workspace
/// `clippy::unwrap_used` deny).
fn lock_menu() -> MutexGuard<'static, ()> {
    MENU_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A diffable snapshot of the daemon state the menu renders. Plain data only, so
/// the per-tick equality check never touches muda objects. Deliberately does NOT
/// include live uptime: it would change every tick and rebuild the menu on a
/// timer (churn, and a rebuild can disrupt the menu if the user has it open), for
/// only a cosmetic label - so the menu rebuilds solely on meaningful state change.
/// `unread` is included (unlike uptime) because it drives the "Mail (N)" label and
/// only changes when mail arrives or is read, not on a timer.
#[derive(Clone, Default, PartialEq, Eq)]
struct TrayState {
    running: bool,
    default_php: Option<String>,
    installed: Vec<String>,
    http: Option<u16>,
    https: Option<u16>,
    /// The Yerd self-update target version, when one is available.
    update_target: Option<String>,
    /// True when any installed PHP version has a newer patch available.
    php_update: bool,
    /// Captured emails not yet marked read.
    unread: u32,
    /// Aggregate health for the icon badge + status header.
    health: TrayHealth,
    /// Running / total managed services (excluding synthetic Proxy/PHP rows).
    services_running: usize,
    services_total: usize,
    /// Color-coded service rows for the menu (Proxy, PHP, Redis, …).
    service_rows: Vec<ServiceRow>,
}

/// User-selectable tray icon appearance; `Auto` (default) keeps today's
/// per-OS behavior (macOS auto-tints a template; other OSes show the full
/// color app icon). Persisted in `gui-settings.json` via
/// `crate::autostart::tray_icon_variant`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TrayIconVariant {
    #[default]
    Auto,
    LightY,
    DarkY,
    Full,
}

/// In-memory cache of the persisted tray icon variant, seeded once from disk
/// at startup (`build_tray`) and kept in sync by [`set_cached_variant`]
/// (called by `crate::autostart::set_tray_icon_variant` right after it saves
/// a change). The poller and refresh paths read this instead of hitting disk
/// (`crate::autostart::tray_icon_variant`, a `std::fs::read` +
/// `serde_json::from_slice`) on every tick.
static CACHED_VARIANT: Mutex<TrayIconVariant> = Mutex::new(TrayIconVariant::Auto);

/// The cached tray icon variant (see [`CACHED_VARIANT`]).
fn cached_variant() -> TrayIconVariant {
    *CACHED_VARIANT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Update the cached tray icon variant; called right after persisting a
/// change so the next poll tick / repaint picks it up without a disk read.
pub(crate) fn set_cached_variant(variant: TrayIconVariant) {
    *CACHED_VARIANT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = variant;
}

/// Build the tray and register it; called once from `setup_app`.
///
/// Hybrid UX: left-click toggles the Vue tray panel; right-click (and the
/// attached menu) shows the native menu. On Linux AppIndicator, clicks often
/// aren't delivered; the native menu's "Jump to site…" item opens the panel.
pub(crate) fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app, &TrayState::default(), dark_menu_bar())?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Yerd")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(|tray, event| {
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                crate::tray_panel::note_tray_icon_rect(&rect);
                let _ = crate::tray_panel::toggle_tray_panel(tray.app_handle());
            }
        });

    let variant = crate::autostart::tray_icon_variant();
    set_cached_variant(variant);
    if let Some(icon) = tray_icon(app, variant, TrayHealth::Bad, dark_menu_bar()) {
        builder = builder.icon(icon);
    }
    #[cfg(target_os = "macos")]
    {
        // Initial icon before the first poll; coloured dot needs non-template mode.
        builder = builder.icon_as_template(false);
    }

    match builder.build(app) {
        Ok(_) => {
            let _ = crate::autostart::set_tray_unavailable(false);
            crate::tray_panel::set_tray_fallback(false);
            Ok(())
        }
        Err(e) => {
            eprintln!("yerd-gui: tray unavailable ({e}); enabling panel fallback");
            let _ = crate::autostart::set_tray_unavailable(true);
            crate::tray_panel::set_tray_fallback(true);
            // Non-fatal: Linux without AppIndicator still runs with the panel.
            Ok(())
        }
    }
}

/// Spawn the background poll loop; called from `setup_app` after `build_tray`.
///
/// Skips while a transition owns the menu, diffs the snapshot to avoid needless
/// rebuilds, and re-checks `TRANSITION` under `MENU_LOCK` before applying so a
/// tick whose fetch overlapped a starting transition can't overwrite its
/// transient menu (see the module concurrency note).
pub(crate) fn spawn_tray_poller(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last: Option<TrayState> = None;
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        loop {
            interval.tick().await;
            if TRANSITION.load(Ordering::Acquire) {
                continue;
            }
            let state = fetch_state().await;
            if last.as_ref() == Some(&state) {
                continue;
            }
            let dark = dark_menu_bar();
            let variant = cached_variant();
            let guard = lock_menu();
            if !TRANSITION.load(Ordering::Acquire) {
                apply(&app, &state, dark, variant);
                last = Some(state);
            }
            drop(guard);
        }
    });
}

/// Fetch daemon status into a snapshot, plus the cached (network-free) update
/// target. An unreachable daemon yields the "stopped" snapshot.
async fn fetch_state() -> TrayState {
    let report = match exchange_timeout(&Request::Status, PROBE_TIMEOUT).await {
        Ok(Response::Status { report }) => report,
        _ => {
            return TrayState {
                health: TrayHealth::Bad,
                ..TrayState::default()
            };
        }
    };
    let update_target = match exchange_timeout(&Request::CachedUpdateStatus, PROBE_TIMEOUT).await {
        Ok(Response::UpdateStatus {
            available: true,
            target,
            ..
        }) => target,
        _ => None,
    };
    tray_state_from_report(*report, update_target)
}

fn tray_state_from_report(report: StatusReport, update_target: Option<String>) -> TrayState {
    let services_total = report.services.len();
    let services_running = report
        .services
        .iter()
        .filter(|s| s.state == ServiceRunState::Running)
        .count();
    let rows = tray_dropdown_service_rows(&report);
    TrayState {
        running: true,
        default_php: Some(report.default_php.to_string()),
        installed: report.php.iter().map(|p| p.version.to_string()).collect(),
        http: Some(report.http.bound),
        https: Some(report.https.bound),
        update_target,
        php_update: report.php.iter().any(|p| p.update_available.is_some()),
        unread: report.mail.as_ref().map_or(0, |m| m.unread),
        health: derive_health(&report),
        services_running,
        services_total,
        service_rows: rows,
    }
}

/// Build + install the menu for `state`, and refresh the tray glyph for the
/// user's chosen `variant`. No lock / no transition logic - callers hold
/// `MENU_LOCK` as needed, and must read `dark_menu_bar()` and the tray icon
/// variant themselves *before* taking it (see the module concurrency note).
/// Status (health / updates / mail) lives in the menu and tray panel; the glyph
/// carries a small health dot at the bottom-right (green / amber / red).
fn apply(app: &AppHandle, state: &TrayState, dark: bool, variant: TrayIconVariant) {
    let Ok(menu) = build_menu(app, state, dark) else {
        return;
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_menu(Some(menu));
        if let Some(icon) = tray_icon(app, variant, state.health, dark) {
            let _ = tray.set_icon(Some(icon));
            #[cfg(target_os = "macos")]
            {
                // Coloured status dot: template mode would flatten it to monochrome.
                let _ = tray.set_icon_as_template(false);
            }
        }
    }
}

/// The tray icon for the user's chosen `variant`.
///
/// `Auto` is today's per-OS default: on macOS the monochrome template
/// (auto-tinted by the OS), on other OSes the full-colour app icon.
/// `LightY`/`DarkY` force the same glyph to a fixed colour on every OS.
/// `Full` is the full-colour app icon on every OS, including macOS.
/// Every variant gets a small health-coloured dot at the bottom-right.
fn tray_icon(
    app: &AppHandle,
    variant: TrayIconVariant,
    health: TrayHealth,
    _dark: bool,
) -> Option<tauri::image::Image<'static>> {
    match variant {
        TrayIconVariant::Full => full_color_icon(app, health),
        TrayIconVariant::LightY | TrayIconVariant::DarkY => {
            let (rgba, w, h) = y_glyph_rgba(variant == TrayIconVariant::LightY, health)?;
            Some(tauri::image::Image::new_owned(rgba, w, h))
        }
        TrayIconVariant::Auto => {
            #[cfg(target_os = "macos")]
            {
                let base = tauri::image::Image::from_bytes(TRAY_ICON_MAC).ok()?;
                let (w, h) = (base.width(), base.height());
                let mut rgba = base.rgba().to_vec();
                // Template mode can't preserve the coloured dot; paint the Y for
                // the current menu-bar appearance instead.
                recolor_opaque(&mut rgba, !_dark);
                overlay_health_dot(&mut rgba, w, h, health);
                Some(tauri::image::Image::new_owned(rgba, w, h))
            }
            #[cfg(not(target_os = "macos"))]
            {
                full_color_icon(app, health)
            }
        }
    }
}

/// The full-colour app icon. Shared by `Full` (every OS) and `Auto` on non-macOS.
fn full_color_icon(app: &AppHandle, health: TrayHealth) -> Option<tauri::image::Image<'static>> {
    let base = app.default_window_icon()?;
    let (w, h) = (base.width(), base.height());
    let mut rgba = base.rgba().to_vec();
    overlay_health_dot(&mut rgba, w, h, health);
    Some(tauri::image::Image::new_owned(rgba, w, h))
}

/// Draw a small health-coloured circle at the bottom-right of a tray icon.
fn overlay_health_dot(rgba: &mut [u8], width: u32, height: u32, health: TrayHealth) {
    if width == 0 || height == 0 {
        return;
    }
    let size = width.min(height);
    let outer_r = ((size as f32) * 0.13).max(4.0).round() as i32;
    let ring = 2i32;
    let inner_r = (outer_r - ring).max(1);
    let cx = width as i32 - outer_r - 2;
    let cy = height as i32 - outer_r - 2;
    let (hr, hg, hb) = health.rgb();
    let outer_r2 = outer_r * outer_r;
    let inner_r2 = inner_r * inner_r;

    let y0 = (cy - outer_r).max(0);
    let y1 = (cy + outer_r).min(height as i32 - 1);
    let x0 = (cx - outer_r).max(0);
    let x1 = (cx + outer_r).min(width as i32 - 1);

    for y in y0..=y1 {
        let dy = y - cy;
        for x in x0..=x1 {
            let dx = x - cx;
            let d2 = dx * dx + dy * dy;
            if d2 > outer_r2 {
                continue;
            }
            let idx = ((y as u32 * width + x as u32) * 4) as usize;
            if d2 <= inner_r2 {
                rgba[idx] = hr;
                rgba[idx + 1] = hg;
                rgba[idx + 2] = hb;
                rgba[idx + 3] = 255;
            } else {
                rgba[idx] = 255;
                rgba[idx + 1] = 255;
                rgba[idx + 2] = 255;
                rgba[idx + 3] = 255;
            }
        }
    }
}

/// Decode the "Y" glyph and force it to solid white (`light`) or solid black.
/// Pure (no `AppHandle`), so it's the part of the `LightY`/`DarkY` icon path
/// that's directly unit-testable.
fn y_glyph_rgba(light: bool, health: TrayHealth) -> Option<(Vec<u8>, u32, u32)> {
    let (mut rgba, w, h) = y_glyph_base_rgba(light)?;
    overlay_health_dot(&mut rgba, w, h, health);
    Some((rgba, w, h))
}

/// Y glyph only (no health dot), shared by [`y_glyph_rgba`] and unit tests.
fn y_glyph_base_rgba(light: bool) -> Option<(Vec<u8>, u32, u32)> {
    let base = tauri::image::Image::from_bytes(TRAY_ICON_MAC).ok()?;
    let (w, h) = (base.width(), base.height());
    let mut rgba = base.rgba().to_vec();
    recolor_opaque(&mut rgba, light);
    Some((rgba, w, h))
}

/// Recolour every opaque pixel in `rgba` to solid white (`to_white`) or solid
/// black, preserving alpha. Shared by [`tray_icon`]'s `LightY`/`DarkY` paths
/// and [`menu_icon`]'s dark-mode recolor.
fn recolor_opaque(rgba: &mut [u8], to_white: bool) {
    let value = if to_white { 255u8 } else { 0u8 };
    for px in rgba.chunks_exact_mut(4) {
        if let [r, g, b, a] = px {
            if *a > 0 {
                (*r, *g, *b) = (value, value, value);
            }
        }
    }
}

/// Whether the system is in Dark mode, so the menu-item glyphs can be painted
/// in the matching colour. Reads the thread-safe `AppleInterfaceStyle` user
/// default, so it's fine off the main thread.
#[cfg(target_os = "macos")]
fn dark_menu_bar() -> bool {
    use objc2_foundation::{NSString, NSUserDefaults};
    let defaults = NSUserDefaults::standardUserDefaults();
    defaults
        .stringForKey(&NSString::from_str("AppleInterfaceStyle"))
        .is_some_and(|s| s.to_string().eq_ignore_ascii_case("dark"))
}

/// Whether the desktop prefers dark mode, so `menu_icon` can paint the glyph in
/// a colour that reads against the theme (muda menu icons aren't templates, and
/// there's no single GTK/Qt API across desktops to ask for the exact label
/// colour). Reads the xdg-desktop-portal `Settings` `color-scheme` preference
/// (GNOME, KDE Plasma, and other portal-backed desktops all implement it) via
/// `dark-light`'s own async-std executor (zbus's async-io backend here, not its
/// `tokio` feature - see the Cargo.toml comment), so its blocking D-Bus round
/// trip (typically sub-25ms against a live portal; unbounded only if the
/// session bus itself is wedged) runs independent of our tokio runtime and is
/// safe to call from the tray poller's worker thread as well as the main
/// thread.
/// Callers take this reading *before* `MENU_LOCK` (see the module concurrency
/// note) so the probe never runs while the lock is held. Any failure (no
/// portal, older desktop) reads as light, which just leaves the glyph at its
/// original black - the current behaviour, so no regression.
#[cfg(target_os = "linux")]
fn dark_menu_bar() -> bool {
    matches!(dark_light::detect(), Ok(dark_light::Mode::Dark))
}

/// No dark-mode signal on other targets (Windows isn't implemented yet) - the
/// menu-item glyphs just stay at their original black, today's behaviour.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn dark_menu_bar() -> bool {
    false
}

/// Install the transient menu shown while a lifecycle action is in flight.
fn apply_transient(app: &AppHandle, label: &str) {
    let Ok(menu) = build_transient_menu(app, label) else {
        return;
    };
    let guard = lock_menu();
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_menu(Some(menu));
    }
    drop(guard);
}

/// Fetch the current state and apply it under the lock unless a transition owns
/// the menu. Used by the non-lifecycle actions (set-default-PHP, update check).
async fn refresh_now(app: &AppHandle) {
    let state = fetch_state().await;
    let dark = dark_menu_bar();
    let variant = cached_variant();
    let guard = lock_menu();
    if !TRANSITION.load(Ordering::Acquire) {
        apply(app, &state, dark, variant);
    }
    drop(guard);
}

/// Spawn [`refresh_now`] in the background; used to repaint the tray
/// immediately after the user changes the tray icon variant in Settings.
pub(crate) fn spawn_refresh(app: AppHandle) {
    tauri::async_runtime::spawn(async move { refresh_now(&app).await });
}

/// Push an owned menu item as a boxed trait object (lets the builder mix
/// `MenuItem`/`CheckMenuItem`/`Submenu`/separators in one list).
fn push(items: &mut Vec<Box<dyn IsMenuItem<Wry>>>, item: impl IsMenuItem<Wry> + 'static) {
    items.push(Box::new(item));
}

fn finish_menu(app: &AppHandle, items: &[Box<dyn IsMenuItem<Wry>>]) -> tauri::Result<Menu<Wry>> {
    let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(AsRef::as_ref).collect();
    Menu::with_items(app, &refs)
}

/// The Mail menu label: "Mail (N)" with the unread count ("99+" over 100), or
/// plain "Mail" when nothing is unread.
fn mail_label(unread: u32) -> String {
    if unread == 0 {
        "Mail".to_string()
    } else if unread > 99 {
        "Mail (99+)".to_string()
    } else {
        format!("Mail ({unread})")
    }
}

/// The full menu for the current daemon state. `dark` is a single
/// `dark_menu_bar()` reading the caller took before taking `MENU_LOCK` (see the
/// module concurrency note), threaded through every icon in this build.
///
/// Layout (v1 hybrid): status header → global actions → Jump to site →
/// services → Mail/Dumps/PHP → footer (Logs / Doctor / Quit).
fn build_menu(app: &AppHandle, state: &TrayState, dark: bool) -> tauri::Result<Menu<Wry>> {
    let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();

    // ── Header ──
    let status_label = if state.running {
        match state.health {
            TrayHealth::Ok => "● Yerd — Running",
            TrayHealth::Warn => "◐ Yerd — Running (degraded)",
            TrayHealth::Bad => "○ Yerd — Running (issues)",
        }
    } else {
        "○ Yerd — Stopped"
    };
    push(&mut items, disabled(app, "noop:header", status_label)?);
    if let Some(php) = &state.default_php {
        push(
            &mut items,
            disabled(app, "noop:phpbadge", format!("PHP {php}"))?,
        );
    }
    if state.running && state.services_total > 0 {
        push(
            &mut items,
            disabled(
                app,
                "noop:svccount",
                format!(
                    "Services {}/{} up",
                    state.services_running, state.services_total
                ),
            )?,
        );
    }
    if let (Some(http), Some(https)) = (state.http, state.https) {
        push(
            &mut items,
            disabled(app, "noop:ports", format!("HTTP :{http} · HTTPS :{https}"))?,
        );
    }
    push(&mut items, PredefinedMenuItem::separator(app)?);

    // ── Global actions ──
    push(
        &mut items,
        action(app, "open", "Open Dashboard", icons::OPEN, dark)?,
    );
    if state.running {
        push(
            &mut items,
            action(app, "services:start-all", "Start All", icons::START, dark)?,
        );
        push(
            &mut items,
            action(app, "services:stop-all", "Stop All", icons::STOP, dark)?,
        );
        push(
            &mut items,
            action(app, "php:restart-all", "Restart PHP", icons::RESTART, dark)?,
        );
    } else {
        push(
            &mut items,
            action(app, "daemon:start", "Start daemon", icons::START, dark)?,
        );
    }
    push(&mut items, PredefinedMenuItem::separator(app)?);

    // ── Sites entry (opens Vue autocomplete panel) ──
    if state.running {
        push(
            &mut items,
            action(app, "jump-site", "Jump to site…", icons::SITES, dark)?,
        );
        push(&mut items, PredefinedMenuItem::separator(app)?);
    }

    // ── Services ──
    if state.running && !state.service_rows.is_empty() {
        push(&mut items, disabled(app, "noop:svclabel", "Services:")?);
        for row in &state.service_rows {
            let label = format!("{} {}", row.health.glyph(), row.label);
            push(
                &mut items,
                MenuItem::with_id(app, format!("svc:{}", row.id), label, true, None::<&str>)?,
            );
        }
        push(&mut items, PredefinedMenuItem::separator(app)?);
    }

    // ── Updates / mail / dumps / site create (preserved) ──
    if state.update_target.is_some() {
        push(
            &mut items,
            action(app, "update:apply", "Update Yerd", icons::UPDATE, dark)?,
        );
    }
    if state.php_update {
        push(
            &mut items,
            action(app, "update:php", "Update PHP", icons::UPDATE_PHP, dark)?,
        );
    }
    if state.running && state.update_target.is_none() {
        push(
            &mut items,
            action(
                app,
                "update:check",
                "Check for updates",
                icons::CHECK_UPDATES,
                dark,
            )?,
        );
    }
    push(
        &mut items,
        action(app, "mail", mail_label(state.unread), icons::MAIL, dark)?,
    );
    push(
        &mut items,
        action(app, "dumps", "Dumps", icons::DUMPS, dark)?,
    );

    if state.running {
        push(&mut items, PredefinedMenuItem::separator(app)?);
        if !state.installed.is_empty() {
            push(&mut items, disabled(app, "noop:phplabel", "Default PHP:")?);
            for v in &state.installed {
                let checked = state.default_php.as_deref() == Some(v.as_str());
                let label = format!("    {}PHP {v}", if checked { "✓ " } else { "  " });
                push(
                    &mut items,
                    MenuItem::with_id(app, format!("php:set:{v}"), label, true, None::<&str>)?,
                );
            }
            push(&mut items, PredefinedMenuItem::separator(app)?);
        }
        push(
            &mut items,
            action(app, "new-site", "New Laravel site…", icons::NEW_SITE, dark)?,
        );
        push(
            &mut items,
            action(app, "sites:link", "Link Site", icons::LINK, dark)?,
        );
        push(
            &mut items,
            action(app, "sites:park", "Park Directory", icons::PARK, dark)?,
        );
        push(&mut items, PredefinedMenuItem::separator(app)?);
        push(
            &mut items,
            action(
                app,
                "daemon:restart",
                "Restart daemon",
                icons::RESTART,
                dark,
            )?,
        );
        push(
            &mut items,
            action(app, "daemon:stop", "Stop daemon", icons::STOP, dark)?,
        );
        push(&mut items, PredefinedMenuItem::separator(app)?);
    }

    // ── Footer ──
    push(
        &mut items,
        action(app, "open-logs", "Open Logs", icons::ABOUT, dark)?,
    );
    push(
        &mut items,
        action(app, "nav:/doctor", "Doctor", icons::DOCTOR, dark)?,
    );
    push(
        &mut items,
        action(app, "quit", "Quit Yerd", icons::QUIT, dark)?,
    );
    finish_menu(app, &items)
}

/// The collapsed menu shown during a daemon transition: a disabled status line
/// plus the always-safe actions. Crucially, **no** daemon-lifecycle items, so a
/// second start/stop/restart can't fire mid-transition and clear `TRANSITION`.
fn build_transient_menu(app: &AppHandle, label: &str) -> tauri::Result<Menu<Wry>> {
    let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();
    let dark = dark_menu_bar();
    push(&mut items, disabled(app, "noop:transient", label)?);
    push(&mut items, PredefinedMenuItem::separator(app)?);
    push(
        &mut items,
        action(app, "open", "Open Yerd", icons::OPEN, dark)?,
    );
    push(&mut items, action(app, "mail", "Mail", icons::MAIL, dark)?);
    push(
        &mut items,
        action(app, "dumps", "Dumps", icons::DUMPS, dark)?,
    );
    push(&mut items, PredefinedMenuItem::separator(app)?);
    push(
        &mut items,
        action(app, "quit", "Quit Yerd", icons::QUIT, dark)?,
    );
    finish_menu(app, &items)
}

/// A clickable item with a leading icon, recoloured per `dark` (see
/// `menu_icon`).
fn action(
    app: &AppHandle,
    id: impl Into<tauri::menu::MenuId>,
    text: impl AsRef<str>,
    icon: &[u8],
    dark: bool,
) -> tauri::Result<IconMenuItem<Wry>> {
    IconMenuItem::with_id(app, id, text, true, menu_icon(icon, dark), None::<&str>)
}

/// Decode a bundled menu-item PNG, recolouring its black glyph to white when
/// `dark` (muda menu icons aren't templates, so they don't auto-tint). Callers
/// pass one `dark_menu_bar()` reading per menu build rather than probing per
/// icon - on Linux that reading is a D-Bus round trip, so this keeps a rebuild
/// at one portal query instead of one per item.
fn menu_icon(png: &[u8], dark: bool) -> Option<tauri::image::Image<'static>> {
    let img = tauri::image::Image::from_bytes(png).ok()?;
    let (w, h) = (img.width(), img.height());
    let mut rgba = img.rgba().to_vec();
    if dark {
        recolor_opaque(&mut rgba, true);
    }
    Some(tauri::image::Image::new_owned(rgba, w, h))
}

/// A non-interactive label (status header / subline).
fn disabled(
    app: &AppHandle,
    id: impl Into<tauri::menu::MenuId>,
    text: impl AsRef<str>,
) -> tauri::Result<MenuItem<Wry>> {
    MenuItem::with_id(app, id, text, false, None::<&str>)
}

/// The single global menu-event handler. It keeps receiving events for items
/// installed later via `set_menu` (the listener is registered on the tray, not
/// the menu instance), so every dynamic id is matched here. Runs on the main
/// thread; it must never take `MENU_LOCK` (it only spawns work or shows windows).
/// `noop:*` labels and unknown ids - including the macOS app menu's
/// `close-window`/`minimize-window`, which share this global event stream - fall
/// through unmatched.
fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id.as_ref();
    match id {
        "open" => {
            crate::show_main(app);
            let _ = app.emit("navigate", "/overview");
        }
        "quit" => app.exit(0),
        "dumps" => {
            let _ = crate::show_dumps(app);
        }
        "mail" => {
            let _ = crate::mail_window::show_mails(app);
        }
        "jump-site" => {
            let _ = crate::tray_panel::show_tray_panel(app);
        }
        "open-logs" => {
            crate::show_main(app);
            let _ = app.emit("navigate", "/about");
            let _ = app.emit("open-logs", ());
        }
        "new-site" => {
            crate::show_main(app);
            let _ = app.emit("sites-intent", "create");
        }
        "sites:link" => {
            crate::show_main(app);
            let _ = app.emit("sites-intent", "link");
        }
        "sites:park" => {
            crate::show_main(app);
            let _ = app.emit("sites-intent", "park");
        }
        "update:apply" => {
            crate::show_main(app);
            let _ = app.emit("navigate", "/about");
        }
        "update:php" => {
            crate::show_main(app);
            let _ = app.emit("navigate", "/php");
        }
        "update:check" => spawn_update_check(app.clone()),
        "daemon:start" => spawn_lifecycle(app.clone(), Lifecycle::Start),
        "daemon:restart" => spawn_lifecycle(app.clone(), Lifecycle::Restart),
        "daemon:stop" => spawn_lifecycle(app.clone(), Lifecycle::Stop),
        "php:restart-all" => spawn_restart_all_php(app.clone()),
        "services:start-all" => spawn_services_all(app.clone(), true),
        "services:stop-all" => spawn_services_all(app.clone(), false),
        _ => {
            if let Some(version) = id.strip_prefix("php:set:") {
                spawn_set_default_php(app.clone(), version.to_string());
            } else if let Some(route) = id.strip_prefix("nav:") {
                crate::show_main(app);
                let _ = app.emit("navigate", route.to_string());
            } else if id.starts_with("svc:") {
                crate::show_main(app);
                let _ = app.emit("navigate", "/services");
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Lifecycle {
    Start,
    Restart,
    Stop,
}

/// Clears `TRANSITION` on drop, so even a panicking lifecycle task or a runtime
/// shutdown mid-action can't leave the tray and poller frozen on "Restarting…".
struct TransitionGuard;
impl Drop for TransitionGuard {
    fn drop(&mut self) {
        TRANSITION.store(false, Ordering::Release);
    }
}

/// Run a daemon lifecycle action while owning the menu: claim `TRANSITION` (a
/// second click while one is in flight is ignored - the transient menu also hides
/// the lifecycle items), show a transient menu, await the bounded settle, then
/// rebuild from fresh state. The final apply clears `TRANSITION` under the lock;
/// the `TransitionGuard` is a backstop that clears it on any abnormal exit.
///
/// Restart waits on a `boot_id` change: the daemon replies `Ok` *before* it
/// re-execs (then the socket briefly drops), so the boot_id change is the only
/// reliable completion signal; the request itself is bounded so a wedged daemon
/// can't hang the task with `TRANSITION` set.
fn spawn_lifecycle(app: AppHandle, kind: Lifecycle) {
    if TRANSITION
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let _guard = TransitionGuard;
        let label = match kind {
            Lifecycle::Start => "Starting…",
            Lifecycle::Restart => "Restarting…",
            Lifecycle::Stop => "Stopping…",
        };
        apply_transient(&app, label);

        match kind {
            Lifecycle::Start => {
                let _ = crate::daemon::start(app.clone(), true).await;
                wait_until_reachable(true).await;
            }
            Lifecycle::Restart => {
                let prev = current_boot_id().await;
                let _ = exchange_timeout(&Request::RestartDaemon, PROBE_TIMEOUT).await;
                wait_until_restarted(prev).await;
            }
            Lifecycle::Stop => {
                let _ = crate::daemon::stop().await;
                wait_until_reachable(false).await;
            }
        }

        let state = fetch_state().await;
        let dark = dark_menu_bar();
        let variant = cached_variant();
        let guard = lock_menu();
        apply(&app, &state, dark, variant);
        TRANSITION.store(false, Ordering::Release);
        drop(guard);
    });
}

/// Apply a tray PHP-version pick (`php:set:{v}`) and refresh the menu. A
/// non-parseable id is ignored.
fn spawn_set_default_php(app: AppHandle, version: String) {
    let Ok(version) = PhpVersion::from_str(&version) else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let _ = exchange(&Request::SetDefaultPhp { version }).await;
        refresh_now(&app).await;
    });
}

fn spawn_restart_all_php(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let _ = exchange(&Request::RestartAllPhp).await;
        refresh_now(&app).await;
    });
}

/// Start or stop every managed service instance that is stopped+enabled (start)
/// or running (stop). Proxy/PHP synthetic rows are not in `ListServices`.
fn spawn_services_all(app: AppHandle, start: bool) {
    tauri::async_runtime::spawn(async move {
        let services = match exchange(&Request::ListServices).await {
            Ok(Response::Services { services }) => services,
            _ => return,
        };
        for s in services {
            let should = if start {
                s.enabled && s.state == ServiceRunState::Stopped
            } else {
                s.state == ServiceRunState::Running
            };
            if !should {
                continue;
            }
            let req = if start {
                Request::StartService {
                    service: s.service.clone(),
                }
            } else {
                Request::StopService {
                    service: s.service.clone(),
                }
            };
            let _ = exchange(&req).await;
        }
        refresh_now(&app).await;
    });
}

/// Run a live (network) self-update check bounded by a timeout - a `None` channel
/// resolves the daemon's persisted preference - then refresh the menu.
async fn run_update_check(app: &AppHandle) {
    let _ = exchange_timeout(
        &Request::CheckUpdate { channel: None },
        Duration::from_secs(20),
    )
    .await;
    refresh_now(app).await;
}

/// Spawn [`run_update_check`] in the background; used by the tray's "Check for
/// updates" menu item.
fn spawn_update_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move { run_update_check(&app).await });
}

/// Fire once at GUI launch (and on a subsequent single-instance re-invoke): if
/// the daemon's last self-update check is already stale (`yerd_update::
/// CHECK_INTERVAL_SECS`, 4h), kick a live check immediately rather than
/// waiting for the daemon's own wall-clock-gated poll to catch up. Silent and
/// non-blocking - an unreachable daemon or fetch failure is swallowed exactly
/// like the daemon's own failure-tolerant polling.
pub(crate) fn spawn_launch_update_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let checked_at = match exchange_timeout(&Request::CachedUpdateStatus, PROBE_TIMEOUT).await {
            Ok(Response::UpdateStatus {
                checked_at_epoch, ..
            }) => checked_at_epoch,
            _ => None,
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if yerd_update::is_check_due(checked_at, now) {
            run_update_check(&app).await;
        }
    });
}

/// Read the daemon's current `boot_id` (the restart-completion key).
async fn current_boot_id() -> Option<u64> {
    match exchange_timeout(&Request::Status, PROBE_TIMEOUT).await {
        Ok(Response::Status { report }) => report.boot_id,
        _ => None,
    }
}

/// Bounded-poll until the daemon's reachability matches `want`.
async fn wait_until_reachable(want: bool) {
    for _ in 0..SETTLE_STEPS {
        tokio::time::sleep(SETTLE_STEP).await;
        let reachable = matches!(
            exchange_timeout(&Request::Status, SETTLE_PROBE_TIMEOUT).await,
            Ok(Response::Status { .. })
        );
        if reachable == want {
            return;
        }
    }
}

/// Bounded-poll until a restart completes: with a known previous `boot_id`, until
/// the daemon is reachable with a *different* `boot_id` (the old process is
/// briefly alive with the old id); against an older daemon that sends no
/// `boot_id`, until an unreachable→reachable transition is observed.
async fn wait_until_restarted(prev: Option<u64>) {
    let mut saw_down = false;
    for _ in 0..SETTLE_STEPS {
        tokio::time::sleep(SETTLE_STEP).await;
        match exchange_timeout(&Request::Status, SETTLE_PROBE_TIMEOUT).await {
            Ok(Response::Status { report }) => match prev {
                Some(old) => {
                    if report.boot_id.is_some_and(|new| new != old) {
                        return;
                    }
                }
                None => {
                    if saw_down {
                        return;
                    }
                }
            },
            _ => saw_down = true,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::{
        icons, menu_icon, overlay_health_dot, recolor_opaque, y_glyph_base_rgba, TrayHealth,
        TrayIconVariant,
    };

    #[test]
    fn menu_icon_light_leaves_pixels_unchanged() {
        let raw = tauri::image::Image::from_bytes(icons::OPEN).expect("bundled icon decodes");
        let light = menu_icon(icons::OPEN, false).expect("bundled icon decodes");
        assert_eq!(light.rgba(), raw.rgba());
    }

    #[test]
    fn menu_icon_dark_recolors_opaque_pixels_white() {
        let dark = menu_icon(icons::OPEN, true).expect("bundled icon decodes");
        for px in dark.rgba().chunks_exact(4) {
            if let [r, g, b, a] = *px {
                if a > 0 {
                    assert_eq!((r, g, b), (255, 255, 255));
                }
            }
        }
    }

    #[test]
    fn menu_icon_dark_preserves_alpha_channel() {
        let raw = tauri::image::Image::from_bytes(icons::OPEN).expect("bundled icon decodes");
        let dark = menu_icon(icons::OPEN, true).expect("bundled icon decodes");
        let raw_alpha: Vec<u8> = raw.rgba().chunks_exact(4).map(|px| px[3]).collect();
        let dark_alpha: Vec<u8> = dark.rgba().chunks_exact(4).map(|px| px[3]).collect();
        assert_eq!(raw_alpha, dark_alpha);
    }

    #[test]
    fn recolor_opaque_to_white_sets_opaque_pixels_white() {
        let mut rgba = vec![10, 20, 30, 255, 40, 50, 60, 0];
        recolor_opaque(&mut rgba, true);
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255]);
        assert_eq!(&rgba[4..8], &[40, 50, 60, 0]);
    }

    #[test]
    fn recolor_opaque_to_black_sets_opaque_pixels_black() {
        let mut rgba = vec![10, 20, 30, 255, 40, 50, 60, 0];
        recolor_opaque(&mut rgba, false);
        assert_eq!(&rgba[0..4], &[0, 0, 0, 255]);
        assert_eq!(&rgba[4..8], &[40, 50, 60, 0]);
    }

    #[test]
    fn y_glyph_rgba_light_is_white() {
        let (rgba, _, _) = y_glyph_base_rgba(true).expect("bundled tray glyph decodes");
        for px in rgba.chunks_exact(4) {
            if let [r, g, b, a] = *px {
                if a > 0 {
                    assert_eq!((r, g, b), (255, 255, 255));
                }
            }
        }
    }

    #[test]
    fn y_glyph_rgba_dark_is_black() {
        let (rgba, _, _) = y_glyph_base_rgba(false).expect("bundled tray glyph decodes");
        for px in rgba.chunks_exact(4) {
            if let [r, g, b, a] = *px {
                if a > 0 {
                    assert_eq!((r, g, b), (0, 0, 0));
                }
            }
        }
    }

    #[test]
    fn overlay_health_dot_paints_green_at_bottom_right() {
        let mut rgba = vec![0u8; 32 * 32 * 4];
        overlay_health_dot(&mut rgba, 32, 32, TrayHealth::Ok);
        let (r, g, b, a) = pixel_at(&rgba, 32, 26, 26);
        assert_eq!((r, g, b, a), (52, 199, 89, 255));
    }

    fn pixel_at(rgba: &[u8], width: u32, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let i = ((y * width + x) * 4) as usize;
        (rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3])
    }

    #[test]
    fn tray_icon_variant_wire_names_are_kebab_case() {
        let cases = [
            (TrayIconVariant::Auto, "\"auto\""),
            (TrayIconVariant::LightY, "\"light-y\""),
            (TrayIconVariant::DarkY, "\"dark-y\""),
            (TrayIconVariant::Full, "\"full\""),
        ];
        for (variant, wire) in cases {
            assert_eq!(
                serde_json::to_string(&variant).expect("enum serializes"),
                wire
            );
        }
    }
}
