//! The separate "Mails" viewer window.
//!
//! The window is declared statically in `tauri.conf.json` (label `mails`,
//! `visible: false`, loading `index.html#/mails-viewer`, sized 1100x760). It is
//! hidden (never destroyed) on close by the window-event handler in `main.rs`, so
//! "show the existing window" is always correct. When it isn't already open we
//! reveal it on the monitor under the cursor (the user's active screen).

use tauri::Manager;

use crate::error::GuiError;

/// The mails window's configured logical size (mirrors `tauri.conf.json`).
const MAILS_SIZE: (f64, f64) = (1100.0, 760.0);

/// Show + focus the Mails viewer window. Shared by the `#[tauri::command]` below
/// (the Mail settings "Show Mails" button) and the tray's "Mail" item, so both
/// open the standalone viewer window rather than the in-app `/mail` config route.
pub(crate) fn show_mails(app: &tauri::AppHandle) -> Result<(), GuiError> {
    let win = app
        .get_webview_window("mails")
        .ok_or_else(|| GuiError::internal("mails window is not configured"))?;
    crate::reveal_aux_window(&win, MAILS_SIZE.0, MAILS_SIZE.1);
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
