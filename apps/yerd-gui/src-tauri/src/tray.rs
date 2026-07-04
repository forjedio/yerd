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
use yerd_ipc::{Request, Response};

use crate::ipc::{exchange, exchange_timeout};

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
    pub const QUIT: &[u8] = include_bytes!("../icons/menu/power.png");
}

/// The navigable pages the tray links to (demoted below the direct actions). PHP
/// is listed inline and Mail/Dumps have their own openers, so they aren't here.
const NAV_ITEMS: &[(&str, &str, &[u8])] = &[
    ("nav:/sites", "Sites", icons::SITES),
    ("nav:/services", "Services", icons::SERVICES),
    ("nav:/about", "About", icons::ABOUT),
];

/// The bare "Y" glyph (solid black stroke, transparent ground), source for the
/// macOS template icon and for the [`TrayIconVariant::LightY`]/`DarkY`
/// overrides on every OS (see `main.rs` for the macOS template rationale).
const TRAY_ICON_MAC: &[u8] = include_bytes!("../icons/tray-mac.png");

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

/// Build the tray and register it; called once from `setup_app`.
///
/// The dynamic menu is the primary surface, so it opens on a plain left-click
/// (the native macOS/Linux menu-bar convention) as well as right-click; the
/// "Open Yerd" item opens the main window. (Windows convention is left-click =
/// open the app; revisit if/when Windows support lands.)
pub(crate) fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app, &TrayState::default(), dark_menu_bar())?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Yerd")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(on_menu_event);

    let variant = crate::autostart::tray_icon_variant();
    let no_badges = Badges {
        update: false,
        unread: false,
    };
    if let Some(icon) = tray_icon(app, no_badges, variant) {
        builder = builder.icon(icon);
    }
    #[cfg(target_os = "macos")]
    {
        builder = builder.icon_as_template(variant == TrayIconVariant::Auto);
    }

    builder.build(app)?;
    Ok(())
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
            let variant = crate::autostart::tray_icon_variant();
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
        _ => return TrayState::default(),
    };
    let update_target = match exchange_timeout(&Request::CachedUpdateStatus, PROBE_TIMEOUT).await {
        Ok(Response::UpdateStatus {
            available: true,
            target,
            ..
        }) => target,
        _ => None,
    };
    TrayState {
        running: true,
        default_php: Some(report.default_php.to_string()),
        installed: report.php.iter().map(|p| p.version.to_string()).collect(),
        http: Some(report.http.bound),
        https: Some(report.https.bound),
        update_target,
        php_update: report.php.iter().any(|p| p.update_available.is_some()),
        unread: report.mail.as_ref().map_or(0, |m| m.unread),
    }
}

/// Which badges the icon needs: a red dot bottom-right for a waiting update (app
/// or PHP), an orange dot bottom-left for unread mail. They can coexist.
#[derive(Clone, Copy)]
struct Badges {
    update: bool,
    unread: bool,
}

impl Badges {
    fn any(self) -> bool {
        self.update || self.unread
    }
}

/// Build + install the menu for `state`, and badge the icon for waiting updates
/// and/or unread mail, in the user's chosen `variant`. No lock / no transition
/// logic - callers hold `MENU_LOCK` as needed, and must read `dark_menu_bar()`
/// and the tray icon variant themselves *before* taking it (see the module
/// concurrency note) since this only threads `dark`/`variant` through. On
/// macOS a coloured badge (or a non-`Auto` variant) can't be a template, so
/// templating only ever applies to the plain `Auto` icon.
fn apply(app: &AppHandle, state: &TrayState, dark: bool, variant: TrayIconVariant) {
    let Ok(menu) = build_menu(app, state, dark) else {
        return;
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_menu(Some(menu));
        let badges = Badges {
            update: state.update_target.is_some() || state.php_update,
            unread: state.unread > 0,
        };
        if let Some(icon) = tray_icon(app, badges, variant) {
            let _ = tray.set_icon(Some(icon));
            #[cfg(target_os = "macos")]
            {
                let _ =
                    tray.set_icon_as_template(variant == TrayIconVariant::Auto && !badges.any());
            }
        }
    }
}

/// Red update dot, drawn bottom-right.
const BADGE_UPDATE: (u8, u8, u8) = (235, 64, 52);
/// Orange unread-mail dot, drawn bottom-left (opposite the update dot).
const BADGE_UNREAD: (u8, u8, u8) = (255, 149, 0);

/// Which bottom corner a badge dot sits in.
#[derive(Clone, Copy)]
enum DotPos {
    /// Bottom-right corner (the update dot).
    BottomRight,
    /// Bottom-left corner (the unread-mail dot).
    BottomLeft,
}

/// The tray icon for the current state + the user's chosen `variant`. Plain
/// icon when nothing's waiting; a copy with a red dot (bottom-right) for a
/// waiting update and/or an orange dot (bottom-left) for unread mail.
///
/// `Auto` is today's per-OS default: on macOS the monochrome template
/// (auto-tinted by the OS when unbadged; a coloured dot can't be a template,
/// so the badged copy is forced to the current appearance's label colour -
/// black in light, white in dark - and `apply` drops templating to match), on
/// other OSes the full-colour app icon. `LightY`/`DarkY` force the same
/// glyph to a fixed colour on every OS, regardless of appearance or badges.
/// `Full` is the full-colour app icon on every OS, including macOS.
fn tray_icon(
    app: &AppHandle,
    badges: Badges,
    variant: TrayIconVariant,
) -> Option<tauri::image::Image<'static>> {
    match variant {
        TrayIconVariant::Full => {
            let base = app.default_window_icon()?;
            let (w, h) = (base.width(), base.height());
            let rgba = base.rgba().to_vec();
            if !badges.any() {
                return Some(tauri::image::Image::new_owned(rgba, w, h));
            }
            Some(draw_badges(rgba, w, h, badges))
        }
        TrayIconVariant::LightY | TrayIconVariant::DarkY => {
            let base = tauri::image::Image::from_bytes(TRAY_ICON_MAC).ok()?;
            let (w, h) = (base.width(), base.height());
            let mut rgba = base.rgba().to_vec();
            recolor_opaque(&mut rgba, variant == TrayIconVariant::LightY);
            if !badges.any() {
                return Some(tauri::image::Image::new_owned(rgba, w, h));
            }
            Some(draw_badges(rgba, w, h, badges))
        }
        TrayIconVariant::Auto => {
            #[cfg(target_os = "macos")]
            {
                let base = tauri::image::Image::from_bytes(TRAY_ICON_MAC).ok()?;
                let (w, h) = (base.width(), base.height());
                let mut rgba = base.rgba().to_vec();
                if !badges.any() {
                    return Some(tauri::image::Image::new_owned(rgba, w, h));
                }
                recolor_opaque(&mut rgba, dark_menu_bar());
                Some(draw_badges(rgba, w, h, badges))
            }
            #[cfg(not(target_os = "macos"))]
            {
                let base = app.default_window_icon()?;
                let (w, h) = (base.width(), base.height());
                let rgba = base.rgba().to_vec();
                if !badges.any() {
                    return Some(tauri::image::Image::new_owned(rgba, w, h));
                }
                Some(draw_badges(rgba, w, h, badges))
            }
        }
    }
}

/// Recolour every opaque pixel in `rgba` to solid white (`to_white`) or solid
/// black, preserving alpha. Shared by [`tray_icon`]'s `LightY`/`DarkY`/badged-
/// `Auto` paths and [`menu_icon`]'s dark-mode recolor.
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

/// Composite the requested dots onto an RGBA buffer: red bottom-right for an
/// update, orange bottom-left for unread mail.
fn draw_badges(
    mut rgba: Vec<u8>,
    width: u32,
    height: u32,
    badges: Badges,
) -> tauri::image::Image<'static> {
    if badges.update {
        draw_dot(&mut rgba, width, height, BADGE_UPDATE, DotPos::BottomRight);
    }
    if badges.unread {
        draw_dot(&mut rgba, width, height, BADGE_UNREAD, DotPos::BottomLeft);
    }
    tauri::image::Image::new_owned(rgba, width, height)
}

/// Paint a small anti-aliased solid dot of `color` onto `rgba`, in the requested
/// bottom corner. (`as` casts and the per-pixel math are inherent to image work.)
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn draw_dot(rgba: &mut [u8], width: u32, height: u32, color: (u8, u8, u8), pos: DotPos) {
    let radius = ((width.min(height) as f32) * 0.16).max(3.0);
    let cy = height as f32 - radius * 1.3;
    let cx = match pos {
        DotPos::BottomRight => width as f32 - radius,
        DotPos::BottomLeft => radius,
    };
    for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
        let dx = (i as u32 % width) as f32 + 0.5 - cx;
        let dy = (i as u32 / width) as f32 + 0.5 - cy;
        let coverage = (radius - dx.hypot(dy) + 0.5).clamp(0.0, 1.0);
        if coverage <= 0.0 {
            continue;
        }
        if let [r, g, b, a] = px {
            let inv = 1.0 - coverage;
            *r = (f32::from(color.0) * coverage + f32::from(*r) * inv) as u8;
            *g = (f32::from(color.1) * coverage + f32::from(*g) * inv) as u8;
            *b = (f32::from(color.2) * coverage + f32::from(*b) * inv) as u8;
            *a = (255.0 * coverage + f32::from(*a) * inv) as u8;
        }
    }
}

/// Whether the system is in Dark mode, so the badged (non-template) icon and
/// the menu-item glyphs can be painted in the matching colour. Reads the
/// thread-safe `AppleInterfaceStyle` user default, so it's fine off the main
/// thread.
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
    let variant = crate::autostart::tray_icon_variant();
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
/// Layout: a top zone (Open Yerd, plus Update Yerd / Update PHP while an update
/// waits) before the status header, then the running- or stopped-daemon block,
/// then Quit. In the running block the installed PHP versions sit under a
/// "Default PHP:" label, each indented with a tick drawn into the label text
/// itself (rather than the fixed native checkmark column) so it nests with the
/// versions; picking one switches the default and the tick moves.
fn build_menu(app: &AppHandle, state: &TrayState, dark: bool) -> tauri::Result<Menu<Wry>> {
    let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();

    push(
        &mut items,
        action(app, "open", "Open Yerd", icons::OPEN, dark)?,
    );
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
    push(&mut items, PredefinedMenuItem::separator(app)?);

    if state.running {
        push(
            &mut items,
            disabled(app, "noop:header", "● Daemon running")?,
        );
        if let (Some(http), Some(https)) = (state.http, state.https) {
            push(
                &mut items,
                disabled(app, "noop:ports", format!("HTTP :{http} · HTTPS :{https}"))?,
            );
        }
        if state.update_target.is_none() {
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
            action(app, "mail", mail_label(state.unread), icons::MAIL, dark)?,
        );
        push(
            &mut items,
            action(app, "dumps", "Dumps", icons::DUMPS, dark)?,
        );
        push(&mut items, PredefinedMenuItem::separator(app)?);
        for (id, label, icon) in NAV_ITEMS {
            push(&mut items, action(app, *id, *label, icon, dark)?);
        }
        push(&mut items, PredefinedMenuItem::separator(app)?);
    } else {
        push(
            &mut items,
            disabled(app, "noop:header", "○ Daemon stopped")?,
        );
        push(
            &mut items,
            action(app, "daemon:start", "Start daemon", icons::START, dark)?,
        );
        push(&mut items, PredefinedMenuItem::separator(app)?);
        push(
            &mut items,
            action(app, "mail", mail_label(state.unread), icons::MAIL, dark)?,
        );
        push(
            &mut items,
            action(app, "dumps", "Dumps", icons::DUMPS, dark)?,
        );
        push(&mut items, PredefinedMenuItem::separator(app)?);
    }

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
        "open" => crate::show_main(app),
        "quit" => app.exit(0),
        "dumps" => {
            let _ = crate::show_dumps(app);
        }
        "mail" => {
            let _ = crate::mail_window::show_mails(app);
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
        _ => {
            if let Some(version) = id.strip_prefix("php:set:") {
                spawn_set_default_php(app.clone(), version.to_string());
            } else if let Some(route) = id.strip_prefix("nav:") {
                crate::show_main(app);
                let _ = app.emit("navigate", route.to_string());
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
        let variant = crate::autostart::tray_icon_variant();
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
    use super::{icons, menu_icon, recolor_opaque};

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
}
