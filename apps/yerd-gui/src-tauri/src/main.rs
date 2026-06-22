// Hide the extra console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod commands;
mod daemon;
mod elevate;
mod error;
mod ipc;
#[cfg(target_os = "macos")]
mod mac_trust;
mod mail_window;

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, WindowEvent,
};
use tauri_plugin_autostart::MacosLauncher;

/// Launch arg the GUI's autostart entry carries (so a login-launched process is
/// distinguishable from a manual open). `tauri-plugin-autostart` fixes the args
/// at `init()` — they can't vary per `enable()` — so this is a constant marker;
/// the *minimized* preference is read separately from `gui-settings.json`.
const AUTOSTART_ARG: &str = "--autostarted";

/// macOS menu-bar tray icon: a monochrome **template** "Y" (see
/// `icons/tray-mac.svg`). Embedded at compile time so it ships in the bundle and
/// is loaded without a runtime path. Template images auto-tint for light/dark
/// and `tray-icon` scales it to the menu-bar's 18pt height, so it sits among the
/// system icons instead of dwarfing them like the full-colour app icon did.
#[cfg(target_os = "macos")]
const TRAY_ICON_MAC: &[u8] = include_bytes!("../icons/tray-mac.png");

fn main() {
    // On Linux/Wayland the dock takes a window's icon from the .desktop file
    // matching its app_id — which GTK derives from the program name. Pin it
    // before GTK initialises so it deterministically matches `yerd-gui.desktop`
    // (installed in dev by scripts/install-dev-desktop.sh, shipped by the .deb).
    #[cfg(target_os = "linux")]
    {
        glib::set_prgname(Some("yerd-gui"));
        glib::set_application_name("Yerd");
    }

    tauri::Builder::default()
        // Must be the first plugin: a second launch focuses the existing
        // window instead of spawning a duplicate (which would risk a duplicate
        // daemon connection / tray).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        // GUI login-autostart. macOS must use a LaunchAgent (the default
        // AppleScript login item can't carry args); the fixed `--autostarted`
        // arg marks login-launched processes.
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARG]),
        ))
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::list_sites,
            commands::park,
            commands::link,
            commands::unlink,
            commands::list_parked,
            commands::unpark,
            commands::set_php,
            commands::set_secure,
            commands::set_web_root,
            commands::list_php,
            commands::check_php_updates,
            commands::available_php,
            commands::install_php,
            commands::set_default_php,
            commands::update_php,
            commands::set_php_settings,
            commands::restart_php,
            commands::restart_all_php,
            commands::uninstall_php,
            commands::restart_daemon,
            commands::list_services,
            commands::available_services,
            commands::install_service,
            commands::change_service_version,
            commands::uninstall_service,
            commands::start_service,
            commands::stop_service,
            commands::restart_service,
            commands::set_service_port,
            commands::service_logs,
            commands::create_database,
            commands::list_databases,
            commands::drop_database,
            commands::backup_database,
            commands::restore_database,
            commands::list_mails,
            commands::get_mail,
            commands::clear_mails,
            commands::delete_mails,
            commands::set_mail_port,
            commands::set_mail_enabled,
            mail_window::show_mails_window,
            commands::status,
            commands::diagnose,
            commands::doctor_fix,
            commands::daemon_info,
            commands::protocol_version,
            commands::host_platform,
            commands::elevate,
            commands::elevate_all,
            commands::unelevate,
            commands::trust_ca,
            commands::untrust_ca,
            commands::list_dumps,
            commands::clear_dumps,
            commands::delete_dump,
            commands::set_dumps_enabled,
            commands::set_dumps_persist,
            commands::set_dumps_port,
            commands::set_dump_feature,
            commands::dumps_status,
            commands::list_tools,
            commands::install_tool,
            commands::uninstall_tool,
            commands::install_tool_streamed,
            commands::create_site,
            commands::job_status,
            commands::job_cancel,
            show_dumps_window,
            daemon::daemon_installed,
            daemon::install_daemon,
            daemon::start_daemon,
            daemon::stop_daemon,
            autostart::get_autostart,
            autostart::set_autostart_daemon,
            autostart::set_autostart_gui,
            autostart::set_gui_minimized,
        ])
        .setup(setup_app)
        // Close-to-tray: hide the window instead of quitting; the tray's Quit
        // item is the real exit.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // The handler is global (fires for every window). The Mails
                // viewer just hides on close — but must NOT touch the Dock
                // activation policy, or closing it would drop the app's Dock
                // icon while the main window is still open. Only the main window
                // drives the close-to-tray + Dock-accessory behaviour.
                let _ = window.hide();
                // Only the main window drops the Dock icon on close — the dumps
                // and Mails viewer windows are auxiliary, so closing one must not
                // yank the main app's presence (or it would minimise the whole
                // app to the tray).
                if window.label() == "main" {
                    set_dock_visible(window.app_handle(), false);
                }
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("yerd-gui: fatal error while running: {e}");
            std::process::exit(1);
        });
}

/// One-time app setup, pulled out of `main`'s builder chain: window icon, the
/// Linux zoom-disable workaround, the tray, and initial window visibility.
fn setup_app(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    set_main_window_icon(app);
    #[cfg(target_os = "linux")]
    disable_webview_zoom(app);
    build_tray(app.handle())?;
    show_initial_window(app);
    Ok(())
}

/// Explicitly set the window icon so the Linux taskbar shows the Yerd mark in
/// dev (no installed .desktop to source it from).
fn set_main_window_icon(app: &tauri::App) {
    if let (Some(win), Some(icon)) = (
        app.get_webview_window("main"),
        app.default_window_icon().cloned(),
    ) {
        let _ = win.set_icon(icon);
    }
}

/// Disable webview zoom on Linux. WebKitGTK handles both gestures below the DOM,
/// so the frontend JS guards can't catch them:
///   - Ctrl+wheel / Ctrl+± change the `zoom-level` property → clamp it.
///   - touchpad pinch is a GtkGestureZoom WebKit installs on its view, which
///     ignores `zoom-level` entirely → remove its handlers.
///
/// (Documented wry workaround, GTK3-only — which is our stack.)
#[cfg(target_os = "linux")]
fn disable_webview_zoom(app: &tauri::App) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let _ = win.with_webview(|pw| {
        use glib::prelude::ObjectExt;
        use webkit2gtk::WebViewExt;
        let wv = pw.inner();

        wv.set_zoom_level(1.0);
        wv.connect_zoom_level_notify(|wv| {
            if (wv.zoom_level() - 1.0).abs() > f64::EPSILON {
                wv.set_zoom_level(1.0);
            }
        });

        // SAFETY: `wk-view-zoom-gesture` is WebKitWebViewBase's internal
        // GtkGestureZoom, stored via `g_object_set_data`. We only destroy its
        // signal handlers so "scale-changed" no longer zooms — we do NOT free the
        // data (which segfaults when JS later prevents events), leaving the object
        // owned by WebKit.
        unsafe {
            if let Some(gesture) = wv.data::<glib::Object>("wk-view-zoom-gesture") {
                glib::gobject_ffi::g_signal_handlers_destroy(gesture.as_ptr().cast());
            }
        }
    });
}

/// Show the main window now — unless this is an autostart launch
/// (`--autostarted`) AND the user chose "start minimized", in which case it stays
/// in the tray. The window is born hidden (`"visible": false` in tauri.conf, to
/// avoid an undecorated flash); a manual open and a non-minimized autostart both
/// show.
fn show_initial_window(app: &tauri::App) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let autostarted = std::env::args().any(|a| a == AUTOSTART_ARG);
    if autostarted && autostart::gui_minimized() {
        // Launched hidden to the tray — start as a menu-bar-only app (no Dock
        // icon) until the user opens the window.
        set_dock_visible(app.handle(), false);
    } else {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// The four navigable views, mirroring the sidebar. Each tray item shows the
/// window and routes the frontend to that page via a `navigate` event.
const NAV_ITEMS: &[(&str, &str)] = &[
    ("nav:/php", "PHP"),
    ("nav:/sites", "Sites"),
    ("nav:/services", "Services"),
    ("nav:/mail", "Mail"),
    ("nav:/about", "About"),
];

/// System tray: open the window, jump to a specific page, or quit.
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Yerd", true, None::<&str>)?;
    let dumps = MenuItem::with_id(app, "dumps", "Show Dumps", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Yerd", true, None::<&str>)?;
    let start = MenuItem::with_id(app, "daemon:start", "Start daemon", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, "daemon:stop", "Stop daemon", true, None::<&str>)?;
    let sep_top = PredefinedMenuItem::separator(app)?;
    let sep_daemon = PredefinedMenuItem::separator(app)?;
    let sep_bottom = PredefinedMenuItem::separator(app)?;

    // Build the per-view "go to page" items, then assemble the menu in order:
    // Open · ─ · PHP · Sites · Services · About · ─ · Start daemon · Stop daemon
    // · ─ · Quit. (Start/Stop are idempotent — both stay enabled.)
    let nav: Vec<MenuItem<_>> = NAV_ITEMS
        .iter()
        .map(|(id, label)| MenuItem::with_id(app, *id, *label, true, None::<&str>))
        .collect::<tauri::Result<_>>()?;
    let mut items: Vec<&dyn tauri::menu::IsMenuItem<_>> = vec![&open, &dumps, &sep_top];
    items.extend(nav.iter().map(|m| m as &dyn tauri::menu::IsMenuItem<_>));
    items.push(&sep_daemon);
    items.push(&start);
    items.push(&stop);
    items.push(&sep_bottom);
    items.push(&quit);
    let menu = Menu::with_items(app, &items)?;

    let mut builder = TrayIconBuilder::with_id("yerd-tray")
        .tooltip("Yerd")
        .menu(&menu)
        // Left-click activates (opens the window) on macOS/Windows; right-click
        // shows the menu. On Linux (AppIndicator) clicks aren't delivered, so the
        // menu items are the way in there.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "dumps" => {
                let _ = show_dumps(app);
            }
            "quit" => app.exit(0),
            // Start/Stop run off the event thread so a slow systemctl/launchctl
            // never stalls the menu; the GUI's status poller reflects the result.
            "daemon:start" => {
                tauri::async_runtime::spawn(async {
                    let _ = daemon::start().await;
                });
            }
            "daemon:stop" => {
                tauri::async_runtime::spawn(async {
                    let _ = daemon::stop().await;
                });
            }
            // "nav:/sites" → show the window and route the frontend to "/sites".
            id => {
                if let Some(route) = id.strip_prefix("nav:") {
                    show_main(app);
                    let _ = app.emit("navigate", route);
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });

    // macOS: a monochrome template glyph sized for the menu bar (see
    // `TRAY_ICON_MAC`). Linux/Windows trays aren't template-based, so keep the
    // full-colour app icon there.
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .icon(tauri::image::Image::from_bytes(TRAY_ICON_MAC)?)
            .icon_as_template(true);
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}

fn show_main(app: &tauri::AppHandle) {
    // Restore the Dock icon (macOS) before showing, so a tray-reopened window
    // gets a normal app presence and can take focus.
    set_dock_visible(app, true);
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Show (or lazily create) the auxiliary "dumps" window — the live Laravel
/// telemetry viewer. Reuses the statically-declared window when it already
/// exists; rebuilds it only if a prior close destroyed it.
fn show_dumps(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(win) = app.get_webview_window("dumps") {
        win.show()?;
        win.set_focus()?;
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        app,
        "dumps",
        tauri::WebviewUrl::App("index.html#/dumps-window".into()),
    )
    .title("Yerd Dumps")
    .inner_size(900.0, 640.0)
    .min_inner_size(640.0, 420.0)
    .decorations(false)
    .transparent(true)
    .build()?;
    Ok(())
}

/// Open the dumps window from the frontend ("Show Dumps" button). Returns the
/// crate's `GuiError` so the frontend sees the same typed `{ code, message }`
/// failure shape as every other command.
#[tauri::command]
fn show_dumps_window(app: tauri::AppHandle) -> Result<(), crate::error::GuiError> {
    show_dumps(&app)
        .map_err(|e| crate::error::GuiError::internal(format!("failed to show dumps window: {e}")))
}

/// Show or hide the app's Dock presence by flipping the macOS activation policy:
/// `Regular` = normal app (Dock icon), `Accessory` = menu-bar-only (no Dock
/// icon, doesn't show as active). Used so closing the window to the tray drops
/// it from the Dock; reopening from the tray brings it back. No-op off macOS.
#[cfg(target_os = "macos")]
fn set_dock_visible(app: &tauri::AppHandle, visible: bool) {
    let policy = if visible {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    };
    let _ = app.set_activation_policy(policy);
    // Re-apply the icon *after* the policy change in both directions: switching
    // policy re-reads the app icon, which is generic in a dev (unbundled) run, so
    // without this the tile flashes the exec icon on hide and shows it on reopen.
    restore_dock_icon();
}

/// Re-apply the embedded Yerd icon to the Dock tile. macOS re-reads the app icon
/// whenever the activation policy changes; in a **dev** run there is no `.app`
/// bundle to source it from, so the tile shows the generic executable icon (Tauri
/// only sets the icon once, at startup, in dev). Release builds carry the bundle
/// icon, so this is gated to debug builds to avoid overriding the
/// higher-resolution bundled `.icns`.
// `lockFocus`/`unlockFocus` are deprecated (resolution-independent drawing) but
// are the simplest way to composite into an `NSImage` without pulling in a
// block-based dependency; they still work on current macOS and this is dev-only.
#[cfg(all(target_os = "macos", debug_assertions))]
#[allow(deprecated)]
fn restore_dock_icon() {
    use objc2::{AllocAnyThread as _, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSCompositingOperation, NSImage};
    use objc2_foundation::{NSData, NSPoint, NSRect, NSSize};

    const APP_ICON_PNG: &[u8] = include_bytes!("../icons/icon.png");
    // Our `icon.png` is full-bleed artwork; real macOS app icons reserve a ~10%
    // transparent margin (the icon grid). Composite onto an inset, transparent
    // canvas so the Dock renders it at the same footprint as its neighbours
    // instead of edge-to-edge (oversized).
    const TILE: f64 = 512.0;
    const MARGIN: f64 = TILE * 0.1;
    const INNER: f64 = TILE - 2.0 * MARGIN;

    // SAFETY: `set_dock_visible` is only called from the main event-loop thread
    // (tray/window-event handlers and `setup`), so a main-thread marker is valid.
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let nsapp = NSApplication::sharedApplication(mtm);

    let data = NSData::with_bytes(APP_ICON_PNG);
    let Some(src) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };

    let canvas = NSImage::initWithSize(NSImage::alloc(), NSSize::new(TILE, TILE));
    canvas.lockFocus();
    src.drawInRect_fromRect_operation_fraction(
        NSRect::new(NSPoint::new(MARGIN, MARGIN), NSSize::new(INNER, INNER)),
        // A zero `fromRect` means "the whole source image".
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
        NSCompositingOperation::SourceOver,
        1.0,
    );
    canvas.unlockFocus();

    // SAFETY: standard AppKit setter; `canvas` is a valid NSImage we just built.
    unsafe { nsapp.setApplicationIconImage(Some(&canvas)) };
}

#[cfg(all(target_os = "macos", not(debug_assertions)))]
fn restore_dock_icon() {}

#[cfg(not(target_os = "macos"))]
fn set_dock_visible(_app: &tauri::AppHandle, _visible: bool) {}
