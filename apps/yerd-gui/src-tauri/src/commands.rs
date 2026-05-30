//! Tauri commands: one per `yerd-ipc` Request, plus a few host-only helpers.
//!
//! Every daemon command maps `command → Request`, calls [`crate::ipc::exchange`],
//! and converts a `Response::Error` into a [`GuiError`] so the frontend only
//! ever sees a success variant or a typed failure. There is no business logic
//! here — that lives in the daemon and its crates (the thin-client rule).

use std::path::PathBuf;

use yerd_core::PhpVersion;
use yerd_ipc::{ErrorCode, Request, Response};

use crate::error::GuiError;
use crate::ipc::exchange;

/// Convert a daemon `Response::Error` into a `GuiError`; pass success through.
fn finish(resp: Response) -> Result<Response, GuiError> {
    if let Response::Error { code, message } = &resp {
        return Err(GuiError::daemon(code_str(code), message.clone()));
    }
    Ok(resp)
}

/// Render an `ErrorCode` as its snake_case wire string (via serde so a new
/// variant doesn't need a match arm here).
fn code_str(code: &ErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "internal".to_owned())
}

// ── liveness ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ping() -> Result<Response, GuiError> {
    finish(exchange(&Request::Ping).await?)
}

// ── sites ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_sites() -> Result<Response, GuiError> {
    finish(exchange(&Request::ListSites).await?)
}

#[tauri::command]
pub async fn park(path: String) -> Result<Response, GuiError> {
    finish(
        exchange(&Request::Park {
            path: PathBuf::from(path),
        })
        .await?,
    )
}

#[tauri::command]
pub async fn link(name: String, path: String) -> Result<Response, GuiError> {
    finish(
        exchange(&Request::Link {
            name,
            path: PathBuf::from(path),
        })
        .await?,
    )
}

#[tauri::command]
pub async fn unlink(name: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::Unlink { name }).await?)
}

#[tauri::command]
pub async fn set_php(name: String, version: PhpVersion) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetPhp { name, version }).await?)
}

#[tauri::command]
pub async fn set_secure(name: String, secure: bool) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetSecure { name, secure }).await?)
}

// ── php versions ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_php() -> Result<Response, GuiError> {
    finish(exchange(&Request::ListPhp).await?)
}

#[tauri::command]
pub async fn check_php_updates() -> Result<Response, GuiError> {
    finish(exchange(&Request::CheckPhpUpdates).await?)
}

#[tauri::command]
pub async fn available_php() -> Result<Response, GuiError> {
    finish(exchange(&Request::AvailablePhp).await?)
}

#[tauri::command]
pub async fn install_php(version: PhpVersion) -> Result<Response, GuiError> {
    finish(exchange(&Request::InstallPhp { version }).await?)
}

#[tauri::command]
pub async fn set_default_php(version: PhpVersion) -> Result<Response, GuiError> {
    finish(exchange(&Request::SetDefaultPhp { version }).await?)
}

#[tauri::command]
pub async fn update_php(version: Option<PhpVersion>) -> Result<Response, GuiError> {
    finish(exchange(&Request::UpdatePhp { version }).await?)
}

// ── status / doctor / info ─────────────────────────────────────────────────

#[tauri::command]
pub async fn status() -> Result<Response, GuiError> {
    finish(exchange(&Request::Status).await?)
}

#[tauri::command]
pub async fn diagnose() -> Result<Response, GuiError> {
    finish(exchange(&Request::Diagnose).await?)
}

#[tauri::command]
pub async fn doctor_fix() -> Result<Response, GuiError> {
    finish(exchange(&Request::DoctorFix).await?)
}

#[tauri::command]
pub async fn daemon_info() -> Result<Response, GuiError> {
    finish(exchange(&Request::DaemonInfo).await?)
}

// ── host-only helpers (no daemon IPC) ──────────────────────────────────────

/// The negotiated IPC protocol version, for the About view.
#[tauri::command]
pub fn protocol_version() -> u32 {
    yerd_ipc::PROTOCOL_VERSION
}

/// The host OS string (`"linux"`, `"macos"`, `"windows"`), to gate platform UI.
#[tauri::command]
pub fn host_platform() -> &'static str {
    std::env::consts::OS
}

/// Run `yerd elevate <target>` under OS elevation. See the plan's elevation
/// section: the GUI never elevates itself; it elevates the audited CLI and
/// threads the real uid through (`pkexec` clears `SUDO_UID`).
#[tauri::command]
pub async fn elevate(target: String) -> Result<(), GuiError> {
    crate::elevate::run(&target).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn finish_passes_success_through() {
        // A non-error response is returned unchanged.
        match finish(Response::Ok) {
            Ok(Response::Ok) => {}
            other => panic!("expected Ok(Response::Ok), got {other:?}"),
        }
        match finish(Response::Sites { sites: vec![] }) {
            Ok(Response::Sites { sites }) => assert!(sites.is_empty()),
            other => panic!("expected Sites, got {other:?}"),
        }
    }

    #[test]
    fn finish_maps_daemon_error_to_gui_error() {
        let err = finish(Response::Error {
            code: ErrorCode::NotFound,
            message: "no such site".to_owned(),
        })
        .unwrap_err();
        assert_eq!(err.code, "not_found");
        assert_eq!(err.message, "no such site");
    }

    #[test]
    fn code_str_renders_snake_case_for_every_known_variant() {
        assert_eq!(code_str(&ErrorCode::NotFound), "not_found");
        assert_eq!(code_str(&ErrorCode::AlreadyExists), "already_exists");
        assert_eq!(code_str(&ErrorCode::InvalidPath), "invalid_path");
        assert_eq!(code_str(&ErrorCode::Internal), "internal");
    }
}
