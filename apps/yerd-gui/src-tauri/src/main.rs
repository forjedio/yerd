// Hide the extra console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod elevate;
mod error;
mod ipc;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager, WindowEvent,
};

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
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::list_sites,
            commands::park,
            commands::link,
            commands::unlink,
            commands::set_php,
            commands::set_secure,
            commands::list_php,
            commands::check_php_updates,
            commands::install_php,
            commands::set_default_php,
            commands::update_php,
            commands::status,
            commands::diagnose,
            commands::doctor_fix,
            commands::daemon_info,
            commands::protocol_version,
            commands::host_platform,
            commands::elevate,
        ])
        .setup(|app| {
            // Explicitly set the window icon so the Linux taskbar shows the Yerd
            // mark in dev (no installed .desktop to source it from).
            if let (Some(win), Some(icon)) =
                (app.get_webview_window("main"), app.default_window_icon().cloned())
            {
                let _ = win.set_icon(icon);
            }
            // Disable webview zoom on Linux. WebKitGTK handles both gestures
            // below the DOM, so the frontend JS guards can't catch them:
            //   - Ctrl+wheel / Ctrl+± change the `zoom-level` property → clamp it.
            //   - touchpad pinch is a GtkGestureZoom WebKit installs on its view,
            //     which ignores `zoom-level` entirely → remove its handlers.
            // (Documented wry workaround, GTK3-only — which is our stack.)
            #[cfg(target_os = "linux")]
            if let Some(win) = app.get_webview_window("main") {
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

                    // SAFETY: `wk-view-zoom-gesture` is WebKitWebViewBase's
                    // internal GtkGestureZoom, stored via `g_object_set_data`. We
                    // only destroy its signal handlers so "scale-changed" no
                    // longer zooms — we do NOT free the data (which segfaults when
                    // JS later prevents events), leaving the object owned by WebKit.
                    unsafe {
                        if let Some(gesture) = wv.data::<glib::Object>("wk-view-zoom-gesture") {
                            glib::gobject_ffi::g_signal_handlers_destroy(gesture.as_ptr().cast());
                        }
                    }
                });
            }
            build_tray(app.handle())?;
            Ok(())
        })
        // Close-to-tray: hide the window instead of quitting; the tray's Quit
        // item is the real exit.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("yerd-gui: fatal error while running: {e}");
            std::process::exit(1);
        });
}

/// A minimal system tray: open the window, or quit.
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Yerd", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    let mut builder = TrayIconBuilder::with_id("yerd-tray")
        .tooltip("Yerd")
        .menu(&menu)
        // Left-click activates (opens the window) on macOS/Windows; right-click
        // shows the menu. On Linux (AppIndicator) clicks aren't delivered, so the
        // menu's "Open Yerd" is the way in there.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
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
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}
