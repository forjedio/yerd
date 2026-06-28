// Hide the extra console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod commands;
mod daemon;
mod elevate;
mod error;
mod ipc;
#[cfg(target_os = "macos")]
mod launch_probe;
mod logging;
#[cfg(target_os = "macos")]
mod mac_trust;
mod mail_window;
#[cfg(target_os = "macos")]
mod smappservice;

#[cfg(target_os = "macos")]
use tauri::menu::{AboutMetadata, Submenu};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, WindowEvent,
};
use tauri_plugin_autostart::MacosLauncher;

/// Launch arg the GUI's autostart entry carries (so a login-launched process is
/// distinguishable from a manual open). `tauri-plugin-autostart` fixes the args
/// at `init()` - they can't vary per `enable()` - so this is a constant marker;
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
    logging::init();

    #[cfg(target_os = "linux")]
    {
        glib::set_prgname(Some("yerd-gui"));
        glib::set_application_name("Yerd");
    }

    #[cfg(target_os = "macos")]
    launch_probe::install_launch_probe();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
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
            commands::install_php_streamed,
            commands::set_default_php,
            commands::update_php,
            commands::check_updates,
            commands::cached_update_status,
            commands::set_update_channel,
            commands::apply_update,
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
            commands::set_fallback_ports,
            commands::set_dns_port,
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
            commands::elevate_resolver_ports,
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
            daemon::daemon_diagnostics,
            daemon::start_daemon,
            daemon::stop_daemon,
            daemon::cli_path_status,
            daemon::install_cli_to_path,
            daemon::remove_cli_from_path,
            daemon::open_login_items,
            autostart::get_autostart,
            autostart::set_autostart_daemon,
            autostart::set_autostart_gui,
            autostart::set_gui_minimized,
            autostart::setup_state,
            autostart::mark_onboarded,
            autostart::daemon_version_conflict,
            logging::gui_log,
            logging::get_gui_logs,
            logging::get_diagnostics,
        ])
        .setup(setup_app)
        // Cmd+W and Cmd+M on a borderless/transparent window: AppKit's
        // performClose: / performMiniaturize: no-op (no closable/miniaturizable
        // style mask), so the custom menu items route here and call the Tauri
        // APIs directly. close() hits the CloseRequested handler below; minimize()
        // works the same as the titlebar control.
        .on_menu_event(|app, event| {
            // get_focused_window is behind Tauri's unstable feature; find the
            // focused window among the managed webview windows instead.
            let focused = app
                .webview_windows()
                .into_values()
                .find(|w| w.is_focused().unwrap_or(false));
            if let Some(win) = focused {
                match event.id.as_ref() {
                    "close-window" => {
                        let _ = win.close();
                    }
                    "minimize-window" => {
                        let _ = win.minimize();
                    }
                    _ => {}
                }
            }
        })
        // Close-to-tray: hide instead of quitting; the tray's Quit item exits.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
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
    #[cfg(target_os = "macos")]
    app.set_menu(build_app_menu(app.handle())?)?;
    build_tray(app.handle())?;
    show_initial_window(app);
    #[cfg(target_os = "macos")]
    std::thread::spawn(|| {
        autostart::migrate_gui_login_if_needed();
        let _ = autostart::ensure_daemon_registration();
    });
    Ok(())
}

/// Build the macOS application menu.
///
/// Mirrors Tauri's default macOS menu but replaces the predefined Close with a
/// custom `close-window` item (Cmd+W). The predefined Close fires AppKit's
/// `performClose:`, which our borderless transparent windows ignore (no closable
/// style mask), so Cmd+W routes through `window.close()` in `on_menu_event`
/// instead, giving the same close-to-tray gesture as the titlebar button.
#[cfg(target_os = "macos")]
fn build_app_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let pkg = app.package_info();
    let config = app.config();
    let about = AboutMetadata {
        name: Some(pkg.name.clone()),
        version: Some(pkg.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };

    // Custom Cmd+W / Cmd+M items (handled in on_menu_event): the predefined
    // Close/Minimize no-op on our borderless windows, so these route to the Tauri
    // close()/minimize() APIs instead.
    let close = MenuItem::with_id(app, "close-window", "Close", true, Some("CmdOrCtrl+W"))?;
    let minimize = MenuItem::with_id(
        app,
        "minimize-window",
        "Minimize",
        true,
        Some("CmdOrCtrl+M"),
    )?;

    let app_menu = Submenu::with_items(
        app,
        pkg.name.clone(),
        true,
        &[
            &PredefinedMenuItem::about(app, None, Some(about))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    let file_menu = Submenu::with_items(app, "File", true, &[&close])?;
    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let view_menu = Submenu::with_items(
        app,
        "View",
        true,
        &[&PredefinedMenuItem::fullscreen(app, None)?],
    )?;
    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &minimize,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &close,
        ],
    )?;

    Menu::with_items(
        app,
        &[&app_menu, &file_menu, &edit_menu, &view_menu, &window_menu],
    )
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
/// (Documented wry workaround, GTK3-only - which is our stack.)
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
        // signal handlers so "scale-changed" no longer zooms - we do NOT free the
        // data (which segfaults when JS later prevents events), leaving the object
        // owned by WebKit.
        unsafe {
            if let Some(gesture) = wv.data::<glib::Object>("wk-view-zoom-gesture") {
                glib::gobject_ffi::g_signal_handlers_destroy(gesture.as_ptr().cast());
            }
        }
    });
}

/// Decide the initial window visibility. On macOS this is **deferred one
/// main-runloop turn** (see [`show_initial_window`]) because the login-launch
/// probe is only authoritative once `applicationDidFinishLaunching` has fully
/// dispatched; elsewhere it runs inline.
///
/// Show the main window - unless this is a login/autostart launch AND the user
/// chose "start minimized", in which case it stays in the tray (Dock hidden).
/// The window is born hidden (`"visible": false` in tauri.conf, to avoid an
/// undecorated flash); a manual open and a non-minimized autostart both show.
fn decide_initial_window(app: &tauri::AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let autostarted = std::env::args().any(|a| a == AUTOSTART_ARG);
    #[cfg(target_os = "macos")]
    let autostarted = autostarted || launch_probe::is_login_launch();
    if autostarted && autostart::gui_minimized() {
        set_dock_visible(app, false);
    } else {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Schedule the initial-window decision. On macOS it must wait until the
/// `applicationDidFinishLaunching` notification has fully dispatched (so the
/// launch probe is set), so it's posted to the main queue via GCD - which always
/// drains on a *later* runloop turn. (Tauri's `run_on_main_thread` runs inline
/// when already on the main thread, which `setup` is, so it would NOT defer.)
fn show_initial_window(app: &tauri::App) {
    let handle = app.handle().clone();
    #[cfg(target_os = "macos")]
    dispatch2::DispatchQueue::main().exec_async(move || decide_initial_window(&handle));
    #[cfg(not(target_os = "macos"))]
    decide_initial_window(&handle);
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
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "dumps" => {
                let _ = show_dumps(app);
            }
            "quit" => app.exit(0),
            "daemon:start" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = daemon::start(app, true).await;
                });
            }
            "daemon:stop" => {
                tauri::async_runtime::spawn(async {
                    let _ = daemon::stop().await;
                });
            }
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
    set_dock_visible(app, true);
    if let Some(win) = app.get_webview_window("main") {
        // Must run before show()/set_focus() so the window lands on the active
        // Space rather than the one it was last shown on.
        #[cfg(target_os = "macos")]
        move_window_to_active_space(&win);
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// macOS: show the main window on the currently active Space instead of pulling
/// the user back to the Space it was last on.
///
/// An NSWindow is bound to one Space, so show() switches Spaces to it. Adding
/// MoveToActiveSpace to its collection behaviour brings it to the active Space on
/// activation. Set on every show (cheap, idempotent) so it survives behaviour
/// resets and applies to windows created before this ran.
#[cfg(target_os = "macos")]
fn move_window_to_active_space(win: &tauri::WebviewWindow) {
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};

    let Ok(ptr) = win.ns_window() else {
        return;
    };
    if ptr.is_null() {
        return;
    }
    // SAFETY: ns_window() returns the window's live NSWindow pointer, and the only
    // show_main callers (tray/window-event handlers) run on the main thread, where
    // AppKit window mutation must happen.
    let ns_window: &NSWindow = unsafe { &*ptr.cast::<NSWindow>() };
    let behavior = ns_window.collectionBehavior() | NSWindowCollectionBehavior::MoveToActiveSpace;
    ns_window.setCollectionBehavior(behavior);
}

/// Show (or lazily create) the auxiliary "dumps" window - the live Laravel
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
