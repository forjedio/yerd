//! Linux graphics workarounds applied before GTK and WebKitGTK initialise.
//!
//! On the NVIDIA driver under Wayland, WebKitGTK's dmabuf renderer trips the
//! driver's explicit-sync path and the compositor drops the connection with
//! `Error 71 (Protocol error) dispatching to Wayland display`: the window
//! flashes for a frame and the process exits. Setting
//! `__NV_DISABLE_EXPLICIT_SYNC=1` avoids it without giving up dmabuf rendering
//! (Tauri's documented first-line fix, cheaper than
//! `WEBKIT_DISABLE_DMABUF_RENDERER=1`).
//!
//! The variable is only set when the session is Wayland, the NVIDIA kernel
//! driver is loaded, and the user has not set it themselves, so an explicit
//! `__NV_DISABLE_EXPLICIT_SYNC=0` still wins.
//!
//! Linux-only on purpose: macOS has no NVIDIA driver or Wayland, so there is
//! no equivalent to mirror.

use std::path::Path;

/// The NVIDIA EGL switch that disables explicit sync for this process tree.
pub const NV_EXPLICIT_SYNC_VAR: &str = "__NV_DISABLE_EXPLICIT_SYNC";

/// Present whenever the NVIDIA DRM kernel module (proprietary or open) is loaded.
const NVIDIA_DRM_MODULE: &str = "/sys/module/nvidia_drm";

/// Host facts the workaround decision depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GfxHost {
    /// The session is Wayland (`WAYLAND_DISPLAY` is set and non-empty).
    pub wayland: bool,
    /// The NVIDIA DRM kernel module is loaded.
    pub nvidia: bool,
    /// The user already set [`NV_EXPLICIT_SYNC_VAR`], to any value.
    pub user_set: bool,
}

/// Whether to set [`NV_EXPLICIT_SYNC_VAR`] for this launch.
pub fn needs_explicit_sync_off(host: GfxHost) -> bool {
    host.wayland && host.nvidia && !host.user_set
}

fn probe_host() -> GfxHost {
    GfxHost {
        wayland: std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()),
        nvidia: Path::new(NVIDIA_DRM_MODULE).exists(),
        user_set: std::env::var_os(NV_EXPLICIT_SYNC_VAR).is_some(),
    }
}

/// Apply the workaround if this host needs it. Must run before any thread is
/// spawned and before GTK loads the EGL driver. Returns whether it was applied
/// so the caller can log it once logging is up.
pub fn apply() -> bool {
    let apply = needs_explicit_sync_off(probe_host());
    if apply {
        std::env::set_var(NV_EXPLICIT_SYNC_VAR, "1");
    }
    apply
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_table() {
        let cases = [
            (true, true, false, true),
            (true, true, true, false),
            (true, false, false, false),
            (false, true, false, false),
            (false, false, false, false),
        ];
        for (wayland, nvidia, user_set, want) in cases {
            let host = GfxHost {
                wayland,
                nvidia,
                user_set,
            };
            assert_eq!(needs_explicit_sync_off(host), want, "{host:?}");
        }
    }
}
