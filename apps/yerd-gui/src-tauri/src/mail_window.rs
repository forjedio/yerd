//! The separate "Mails" viewer window.
//!
//! The window is declared statically in `tauri.conf.json` (label `mails`,
//! `visible: false`, loading `index.html#/mails-viewer`), so this command just
//! shows-and-focuses it - mirroring `show_main`. It is hidden (never destroyed)
//! on close by the window-event handler in `main.rs`, so "show the existing
//! window" is always correct.

use tauri::Manager;

use crate::error::GuiError;

/// Show + focus the Mails viewer window. Shared by the `#[tauri::command]` below
/// (the Mail settings "Show Mails" button) and the tray's "Mail" item, so both
/// open the standalone viewer window rather than the in-app `/mail` config route.
pub(crate) fn show_mails(app: &tauri::AppHandle) -> Result<(), GuiError> {
    let win = app
        .get_webview_window("mails")
        .ok_or_else(|| GuiError::internal("mails window is not configured"))?;
    win.show().map_err(|e| GuiError::internal(e.to_string()))?;
    win.set_focus()
        .map_err(|e| GuiError::internal(e.to_string()))?;
    Ok(())
}

/// Open (show + focus) the Mails viewer window. Invoked from the Mail settings
/// page's "Show Mails" button. Returns the crate's single `GuiError` type so the
/// frontend sees the same typed `{ code, message }` failure shape as every other
/// command.
#[tauri::command]
pub fn show_mails_window(app: tauri::AppHandle) -> Result<(), GuiError> {
    show_mails(&app)
}
