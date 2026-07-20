//! Preferred IDE detection and launch for tray / site "Open in IDE".
//!
//! Catalog is shared across macOS and Linux; only install checks and launch
//! commands differ. Preference is stored in `gui-settings.json` via
//! [`crate::autostart`].

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::GuiError;

/// One entry in the Settings "Preferred IDE" list.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdeInfo {
    pub id: String,
    pub label: String,
    pub installed: bool,
}

struct IdeSpec {
    id: &'static str,
    label: &'static str,
    /// macOS `.app` bundle name (without `.app`).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    macos_app: &'static str,
    /// Linux CLI binary name on `PATH`.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    linux_bin: &'static str,
}

const CATALOG: &[IdeSpec] = &[
    IdeSpec {
        id: "cursor",
        label: "Cursor",
        macos_app: "Cursor",
        linux_bin: "cursor",
    },
    IdeSpec {
        id: "vscode",
        label: "Visual Studio Code",
        macos_app: "Visual Studio Code",
        linux_bin: "code",
    },
    IdeSpec {
        id: "phpstorm",
        label: "PhpStorm",
        macos_app: "PhpStorm",
        linux_bin: "phpstorm",
    },
    IdeSpec {
        id: "sublime",
        label: "Sublime Text",
        macos_app: "Sublime Text",
        linux_bin: "subl",
    },
    IdeSpec {
        id: "zed",
        label: "Zed",
        macos_app: "Zed",
        linux_bin: "zed",
    },
];

const SYSTEM_ID: &str = "system";

/// All known IDEs plus "System default", with live install flags.
pub fn list_ides() -> Vec<IdeInfo> {
    let mut out: Vec<IdeInfo> = CATALOG
        .iter()
        .map(|s| IdeInfo {
            id: s.id.to_string(),
            label: s.label.to_string(),
            installed: is_installed(s),
        })
        .collect();
    out.push(IdeInfo {
        id: SYSTEM_ID.to_string(),
        label: "System default".to_string(),
        installed: true,
    });
    out
}

/// Resolve a stored preference (`""` = auto) to a concrete IDE id to launch.
pub fn resolve_ide_id(preferred: &str) -> String {
    let pref = preferred.trim();
    if pref.is_empty() {
        return first_installed_or_system();
    }
    if pref == SYSTEM_ID {
        return SYSTEM_ID.to_string();
    }
    if let Some(spec) = CATALOG.iter().find(|s| s.id == pref) {
        if is_installed(spec) {
            return spec.id.to_string();
        }
    }
    // Missing / uninstalled preference → auto, then system.
    first_installed_or_system()
}

fn first_installed_or_system() -> String {
    CATALOG
        .iter()
        .find(|s| is_installed(s))
        .map(|s| s.id.to_string())
        .unwrap_or_else(|| SYSTEM_ID.to_string())
}

fn is_installed(spec: &IdeSpec) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos_app_exists(spec.macos_app)
    }
    #[cfg(target_os = "linux")]
    {
        which_bin(spec.linux_bin).is_some()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = spec;
        false
    }
}

#[cfg(target_os = "macos")]
fn macos_app_exists(name: &str) -> bool {
    let mut candidates = vec![PathBuf::from(format!("/Applications/{name}.app"))];
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join("Applications")
                .join(format!("{name}.app")),
        );
    }
    for c in &candidates {
        if c.is_dir() {
            return true;
        }
    }
    // JetBrains Toolbox sometimes uses versioned names; accept any PhpStorm*.app.
    if name == "PhpStorm" {
        if let Ok(rd) = std::fs::read_dir("/Applications") {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.starts_with("PhpStorm") && n.ends_with(".app") {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(target_os = "macos")]
fn macos_app_name_for(id: &str) -> Option<&'static str> {
    CATALOG.iter().find(|s| s.id == id).map(|s| s.macos_app)
}

#[cfg(target_os = "linux")]
fn linux_bin_for(id: &str) -> Option<&'static str> {
    CATALOG.iter().find(|s| s.id == id).map(|s| s.linux_bin)
}

#[cfg(target_os = "linux")]
fn which_bin(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Open `path` in the resolved IDE (or system file handler).
pub fn open_path_in_ide(path: &Path, preferred: &str) -> Result<(), GuiError> {
    if !path.exists() {
        return Err(GuiError::internal(format!(
            "path does not exist: {}",
            path.display()
        )));
    }
    let id = resolve_ide_id(preferred);
    match open_with_id(path, &id) {
        Ok(()) => Ok(()),
        Err(e) if id != SYSTEM_ID => {
            // Preferred missing at launch time — fall back to system.
            let _ = e;
            open_with_id(path, SYSTEM_ID)
        }
        Err(e) => Err(e),
    }
}

fn open_with_id(path: &Path, id: &str) -> Result<(), GuiError> {
    if id == SYSTEM_ID {
        return open_system(path);
    }
    #[cfg(target_os = "macos")]
    {
        let app = macos_app_name_for(id)
            .ok_or_else(|| GuiError::internal(format!("unknown IDE id: {id}")))?;
        // Prefer exact /Applications path when present (Toolbox versioned names).
        let mut cmd = Command::new("open");
        if id == "phpstorm" {
            if let Some(bundle) = find_phpstorm_bundle() {
                cmd.args(["-a"]).arg(&bundle).arg(path);
            } else {
                cmd.args(["-a", app]).arg(path);
            }
        } else {
            cmd.args(["-a", app]).arg(path);
        }
        let status = cmd
            .status()
            .map_err(|e| GuiError::internal(format!("open -a {app} failed: {e}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(GuiError::internal(format!(
                "open -a {app} exited with {status}"
            )))
        }
    }
    #[cfg(target_os = "linux")]
    {
        let bin =
            linux_bin_for(id).ok_or_else(|| GuiError::internal(format!("unknown IDE id: {id}")))?;
        // Editors typically daemonize; don't wait on exit status.
        Command::new(bin)
            .arg(path)
            .spawn()
            .map_err(|e| GuiError::internal(format!("{bin} failed: {e}")))?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (path, id);
        Err(GuiError::internal(
            "open_site_in_ide is unsupported on this OS",
        ))
    }
}

#[cfg(target_os = "macos")]
fn find_phpstorm_bundle() -> Option<PathBuf> {
    let exact = PathBuf::from("/Applications/PhpStorm.app");
    if exact.is_dir() {
        return Some(exact);
    }
    let rd = std::fs::read_dir("/Applications").ok()?;
    let mut matches: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            n.starts_with("PhpStorm") && n.ends_with(".app")
        })
        .collect();
    matches.sort();
    matches.pop()
}

fn open_system(path: &Path) -> Result<(), GuiError> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("open")
            .arg(path)
            .status()
            .map_err(|e| GuiError::internal(format!("open failed: {e}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(GuiError::internal(format!("open exited with {status}")))
        }
    }
    #[cfg(target_os = "linux")]
    {
        let status = Command::new("xdg-open")
            .arg(path)
            .status()
            .map_err(|e| GuiError::internal(format!("xdg-open failed: {e}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(GuiError::internal(format!("xdg-open exited with {status}")))
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
        Err(GuiError::internal(
            "open_site_in_ide is unsupported on this OS",
        ))
    }
}

/// Whether `id` is a known preference value (`""` / catalog id / `system`).
pub fn is_valid_preference(id: &str) -> bool {
    let id = id.trim();
    id.is_empty() || id == SYSTEM_ID || CATALOG.iter().any(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_empty_pref_returns_system_or_installed() {
        let id = resolve_ide_id("");
        assert!(id == SYSTEM_ID || CATALOG.iter().any(|s| s.id == id));
    }

    #[test]
    fn resolve_system_stays_system() {
        assert_eq!(resolve_ide_id("system"), SYSTEM_ID);
    }

    #[test]
    fn resolve_unknown_falls_back() {
        let id = resolve_ide_id("not-a-real-ide");
        assert!(id == SYSTEM_ID || CATALOG.iter().any(|s| s.id == id));
    }

    #[test]
    fn list_includes_system() {
        let list = list_ides();
        assert!(list.iter().any(|i| i.id == SYSTEM_ID && i.installed));
        assert!(list.len() > CATALOG.len());
    }

    #[test]
    fn valid_preferences() {
        assert!(is_valid_preference(""));
        assert!(is_valid_preference("system"));
        assert!(is_valid_preference("cursor"));
        assert!(!is_valid_preference("notepad"));
    }
}
