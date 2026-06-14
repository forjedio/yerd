//! The separate "Mails" viewer window.
//!
//! The window is declared statically in `tauri.conf.json` (label `mails`,
//! `visible: false`, loading `index.html#/mails-viewer`), so this command just
//! shows-and-focuses it — mirroring `show_main`. It is hidden (never destroyed)
//! on close by the window-event handler in `main.rs`, so "show the existing
//! window" is always correct.

use tauri::Manager;

/// Open (show + focus) the Mails viewer window. Invoked from the Mail settings
/// page's "Show Mails" button.
#[tauri::command]
pub fn show_mails_window(app: tauri::AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("mails")
        .ok_or_else(|| "mails window is not configured".to_string())?;
    win.show().map_err(|e| e.to_string())?;
    win.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}
