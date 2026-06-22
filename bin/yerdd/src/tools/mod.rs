//! Dev-tool installer subsystem — Composer, Node (node/npm/npx), Bun (bun/bunx).
//!
//! Each tool ships as a self-contained, relocatable binary (no global install):
//! Node's tarball, Bun's zip, Composer's phar. yerd downloads + sha256-verifies
//! the latest release into `{data}/tools/<id>/` and symlinks the commands it
//! provides into `{data}/bin` (on `PATH` via `yerd path`). Same I/O-edge pattern
//! as `php_install`/`ext_install`: a `Downloader` trait is injected; the pure
//! resolution bits are inline + unit-tested.

pub mod bun;
pub mod composer;
pub mod laravel;
pub mod node;

use std::path::{Path, PathBuf};

use yerd_ipc::ToolStatus;
use yerd_php::Downloader;
use yerd_platform::PlatformDirs;

use crate::ext_install::sha256_hex;

/// The dev tools yerd can install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Composer (PHP dependency manager) — a phar run via the managed PHP.
    Composer,
    /// Node.js — `node`, `npm`, `npx`.
    Node,
    /// Bun — `bun`, `bunx`.
    Bun,
    /// The Laravel installer (`laravel new`) — a Composer package run via the
    /// managed PHP, exposed as the `laravel` multi-call shim.
    Laravel,
}

/// Filename of the installed-version marker inside a tool's dir.
const VERSION_MARKER: &str = ".version";

impl Tool {
    /// Every tool, for `list_status` / reconcile.
    pub const ALL: [Tool; 4] = [Tool::Composer, Tool::Node, Tool::Bun, Tool::Laravel];

    /// Stable id used on the wire and as the on-disk dir name.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Tool::Composer => "composer",
            Tool::Node => "node",
            Tool::Bun => "bun",
            Tool::Laravel => "laravel",
        }
    }

    /// Human-readable name for the UI.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Tool::Composer => "Composer",
            Tool::Node => "Node.js",
            Tool::Bun => "Bun",
            Tool::Laravel => "Laravel Installer",
        }
    }

    /// The commands this tool exposes in `{data}/bin`.
    #[must_use]
    pub const fn exposed_bins(self) -> &'static [&'static str] {
        match self {
            Tool::Composer => &["composer"],
            Tool::Node => &["node", "npm", "npx"],
            Tool::Bun => &["bun", "bunx"],
            Tool::Laravel => &["laravel"],
        }
    }

    /// Parse a wire id back to a `Tool`.
    #[must_use]
    pub fn parse(id: &str) -> Option<Tool> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }
}

/// Failure modes of a tool install. Mapped to `Response::Error` by the dispatch
/// arm (see `ipc_server::tool_error_code`).
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// Network / HTTP failure fetching a release artifact or index.
    #[error("download failed: {0}")]
    Download(String),
    /// A downloaded artifact's SHA-256 did not match its published sidecar.
    #[error("integrity check failed: {0}")]
    Sha256Mismatch(String),
    /// Unpacking the archive (tar/zip) failed, or its layout was unexpected.
    #[error("unpack failed: {0}")]
    Unpack(String),
    /// No prebuilt artifact is published for this OS/arch.
    #[error("{0} is not available for this platform")]
    UnsupportedHost(&'static str),
    /// A filesystem operation failed.
    #[error("{0}")]
    Io(String),
    /// The requested tool id is not one yerd manages.
    #[error("unknown tool {0:?}")]
    Unknown(String),
}

/// `{data}/tools/<id>`.
pub(crate) fn tool_dir(dirs: &PlatformDirs, tool: Tool) -> PathBuf {
    dirs.data.join("tools").join(tool.id())
}

/// `{data}/bin`.
pub(crate) fn bin_dir(dirs: &PlatformDirs) -> PathBuf {
    dirs.data.join("bin")
}

/// Read a tool's installed version from its `.version` marker, or `None`.
pub(crate) fn installed_version(dirs: &PlatformDirs, tool: Tool) -> Option<String> {
    let v = std::fs::read_to_string(tool_dir(dirs, tool).join(VERSION_MARKER)).ok()?;
    let v = v.trim().to_owned();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Status of one tool (installed + version + the commands it provides).
#[must_use]
pub fn status(dirs: &PlatformDirs, tool: Tool) -> ToolStatus {
    let version = installed_version(dirs, tool);
    ToolStatus {
        id: tool.id().to_owned(),
        display_name: tool.display_name().to_owned(),
        installed: version.is_some(),
        version,
        binaries: tool
            .exposed_bins()
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
    }
}

/// Status of every tool, for `Response::Tools`. Pure fs reads (no network/lock).
#[must_use]
pub fn list_status(dirs: &PlatformDirs) -> Vec<ToolStatus> {
    Tool::ALL.iter().map(|&t| status(dirs, t)).collect()
}

/// A sink for streamed install output (one line per send). The streamed-install
/// job drains it into the job log; the blocking path passes `None`.
pub type ProgressTx = tokio::sync::mpsc::UnboundedSender<String>;

/// Emit one progress line if a sink is attached.
fn note(progress: Option<&ProgressTx>, msg: impl Into<String>) {
    if let Some(tx) = progress {
        let _ = tx.send(msg.into());
    }
}

/// Download + install `tool`'s latest release. Idempotent (replaces in place via
/// staging + atomic swap). Best-effort integrity is sha256-verified per asset.
/// When `progress` is set, coarse status (and, for the Laravel installer, the
/// live Composer output) is streamed to it.
pub async fn install(
    tool: Tool,
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    progress: Option<&ProgressTx>,
) -> Result<(), ToolError> {
    note(progress, format!("Installing {}…", tool.display_name()));
    let result = match tool {
        Tool::Composer => composer::install(dirs, dl).await,
        Tool::Node => node::install(dirs, dl).await,
        Tool::Bun => bun::install(dirs, dl).await,
        // The installer is a Composer package, not a downloadable artifact, so it
        // ignores `dl` and drives the managed Composer itself — streaming its output.
        Tool::Laravel => laravel::install(dirs, progress).await,
    };
    match &result {
        Ok(()) => note(progress, format!("Installed {}", tool.display_name())),
        Err(e) => note(progress, format!("Error: {e}")),
    }
    result
}

/// Remove `tool`'s files. The `{data}/bin` shims are pruned by a subsequent
/// `reconcile_tool_shims` (the caller runs it under the shim mutex).
pub fn uninstall(dirs: &PlatformDirs, tool: Tool) -> Result<(), ToolError> {
    let d = tool_dir(dirs, tool);
    if d.exists() {
        std::fs::remove_dir_all(&d).map_err(|e| ToolError::Io(format!("{}: {e}", d.display())))?;
    }
    Ok(())
}

/// The `(name_in_bin, symlink_target)` pairs for an installed `tool`. Empty if
/// the tool isn't installed or its root can't be resolved.
#[cfg(unix)]
fn shim_links(dirs: &PlatformDirs, tool: Tool, yerd_bin: &Path) -> Vec<(String, PathBuf)> {
    match tool {
        // Composer runs via the managed PHP — its `composer` command is the
        // multi-call shim into the `yerd` binary, like the cover shims.
        Tool::Composer => vec![("composer".to_owned(), yerd_bin.to_path_buf())],
        Tool::Node => node::shim_links(dirs),
        Tool::Bun => bun::shim_links(dirs),
        // The `laravel` command is likewise a multi-call shim into `yerd`.
        Tool::Laravel => vec![("laravel".to_owned(), yerd_bin.to_path_buf())],
    }
}

/// Reconcile `{data}/bin` tool shims against what's installed: (re)create the
/// commands of installed tools, prune those of uninstalled tools. **Prunes by
/// name-ownership** (gated on `is_symlink`, never `target.exists()`), so a
/// dangling link after uninstall is still removed. Callers must hold the shared
/// `shim_reconcile` mutex (this writes the same dir as `php_install::reconcile_shims`).
/// Unix-only; no-op elsewhere.
#[cfg(unix)]
pub fn reconcile_tool_shims(dirs: &PlatformDirs, yerd_bin: &Path) -> Result<(), ToolError> {
    let bin = bin_dir(dirs);
    std::fs::create_dir_all(&bin).map_err(|e| ToolError::Io(format!("{}: {e}", bin.display())))?;

    for &tool in &Tool::ALL {
        if installed_version(dirs, tool).is_some() {
            for (name, target) in shim_links(dirs, tool, yerd_bin) {
                crate::php_install::place_symlink(&bin.join(&name), &target)
                    .map_err(|e| ToolError::Io(e.to_string()))?;
            }
        } else {
            // Prune this tool's owned names if present as a symlink.
            for &name in tool.exposed_bins() {
                let p = bin.join(name);
                let is_link =
                    std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink());
                if is_link {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn reconcile_tool_shims(_dirs: &PlatformDirs, _yerd_bin: &Path) -> Result<(), ToolError> {
    Ok(())
}

// ---- shared helpers used by the per-tool modules ----

/// Select the SHA-256 hex for `exact_filename` from a `SHASUMS256.txt` body
/// (`"<hex>  <filename>"` lines, GNU coreutils format). Node and Bun publish
/// **many** assets per release (platform + `baseline`/`musl`/`profile` decoys),
/// so the match must be on the *exact* filename, not a substring. Tolerates
/// CRLF, a UTF-8 BOM on line 1, and a `*` binary-mode marker; lowercases the hex.
pub(crate) fn sha_for_asset(sums_text: &str, exact_filename: &str) -> Option<String> {
    for raw in sums_text.lines() {
        let line = raw.trim_start_matches('\u{feff}').trim();
        let mut parts = line.split_whitespace();
        let hex = parts.next()?;
        // The filename is the remainder; coreutils uses `<hex>  <name>` (a `*`
        // prefix marks binary mode). Take the last token and strip a leading `*`.
        let Some(name) = parts.last() else { continue };
        let name = name.strip_prefix('*').unwrap_or(name);
        if name == exact_filename && hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(hex.to_ascii_lowercase());
        }
    }
    None
}

/// Verify `bytes` against `want_sha` (hex). `label` names the asset for errors.
pub(crate) fn verify_sha256(bytes: &[u8], want_sha: &str, label: &str) -> Result<(), ToolError> {
    let got = sha256_hex(bytes);
    if got.eq_ignore_ascii_case(want_sha) {
        Ok(())
    } else {
        Err(ToolError::Sha256Mismatch(format!(
            "{label}: got {got}, want {want_sha}"
        )))
    }
}

/// The single child **directory** of `dir` (Node/Bun archives wrap their payload
/// in one top-level dir whose name encodes the version). Errors unless exactly
/// one directory entry exists — never reconstructed from a version string.
pub(crate) fn extract_root_dir(dir: &Path) -> Result<PathBuf, ToolError> {
    let mut found: Option<PathBuf> = None;
    let entries =
        std::fs::read_dir(dir).map_err(|e| ToolError::Unpack(format!("{}: {e}", dir.display())))?;
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            if found.is_some() {
                return Err(ToolError::Unpack(format!(
                    "expected one top-level dir in {}, found multiple",
                    dir.display()
                )));
            }
            found = Some(entry.path());
        }
    }
    found.ok_or_else(|| ToolError::Unpack(format!("no top-level dir in {}", dir.display())))
}

/// Stage `tool`'s payload via a fresh staging dir, write the `.version` marker,
/// then atomically swap it into `{data}/tools/<id>` (mirrors `php_install::install`,
/// so an update replaces in place and leaves exactly one versioned child).
/// `unpack` lays the artifact's contents into the staging dir.
pub(crate) fn stage_and_swap(
    dirs: &PlatformDirs,
    tool: Tool,
    version: &str,
    unpack: impl FnOnce(&Path) -> Result<(), ToolError>,
) -> Result<(), ToolError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Monotonic per-call counter so two overlapping installs/updates of the same
    // tool in this process can't race on the same staging/backup paths.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);

    let tools_root = dirs.data.join("tools");
    std::fs::create_dir_all(&tools_root)
        .map_err(|e| ToolError::Io(format!("{}: {e}", tools_root.display())))?;
    let staging = tools_root.join(format!(
        ".staging-{}-{}-{seq}",
        tool.id(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|e| ToolError::Io(format!("{}: {e}", staging.display())))?;

    let result = (|| {
        unpack(&staging)?;
        let marker = staging.join(VERSION_MARKER);
        std::fs::write(&marker, version)
            .map_err(|e| ToolError::Io(format!("{}: {e}", marker.display())))
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    // Move the current install aside (rather than deleting it) so a failure
    // between the swap-out and swap-in leaves the previous, still-valid payload
    // recoverable instead of uninstalling the tool.
    let final_dir = tool_dir(dirs, tool);
    let backup = tools_root.join(format!(
        ".previous-{}-{}-{seq}",
        tool.id(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&backup);
    if final_dir.exists() {
        if let Err(e) = std::fs::rename(&final_dir, &backup) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(ToolError::Io(format!("{}: {e}", final_dir.display())));
        }
    }
    if let Err(e) = std::fs::rename(&staging, &final_dir) {
        // Roll the previous install back into place before surfacing the error.
        if backup.exists() {
            let _ = std::fs::rename(&backup, &final_dir);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(ToolError::Io(format!("{}: {e}", final_dir.display())));
    }
    let _ = std::fs::remove_dir_all(&backup);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_ids_round_trip() {
        for t in Tool::ALL {
            assert_eq!(Tool::parse(t.id()), Some(t));
        }
        assert_eq!(Tool::parse("yarn"), None);
    }

    #[test]
    fn sha_for_asset_matches_exact_filename_among_decoys() {
        // Mixed: CRLF, BOM on line 1, a binary `*` marker, and decoy variants.
        let h = "a".repeat(64);
        let want = "b".repeat(64);
        let body = format!(
            "\u{feff}{h}  bun-linux-x64-baseline.zip\r\n\
             {h}  bun-linux-x64-musl.zip\n\
             {want} *bun-linux-x64.zip\n\
             {h}  bun-linux-x64-profile.zip\n",
        );
        assert_eq!(
            sha_for_asset(&body, "bun-linux-x64.zip").as_deref(),
            Some(want.as_str())
        );
        // A platform we didn't list is absent.
        assert_eq!(sha_for_asset(&body, "bun-darwin-aarch64.zip"), None);
    }

    #[test]
    fn sha_for_asset_rejects_non_hex() {
        let body = "nothex  node-v1-darwin-arm64.tar.gz\n";
        assert_eq!(sha_for_asset(body, "node-v1-darwin-arm64.tar.gz"), None);
    }

    fn dirs_in(tmp: &std::path::Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    #[test]
    fn status_reflects_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        // Not installed.
        let s = status(&dirs, Tool::Node);
        assert!(!s.installed);
        assert_eq!(s.binaries, vec!["node", "npm", "npx"]);
        // Write a marker → installed with version.
        let d = tool_dir(&dirs, Tool::Node);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(VERSION_MARKER), "v24.17.0").unwrap();
        let s = status(&dirs, Tool::Node);
        assert!(s.installed);
        assert_eq!(s.version.as_deref(), Some("v24.17.0"));
    }

    #[test]
    fn extract_root_dir_requires_exactly_one() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(extract_root_dir(root).is_err()); // none
        std::fs::create_dir(root.join("node-v24.17.0-darwin-arm64")).unwrap();
        std::fs::write(root.join(".version"), "x").unwrap(); // a file is ignored
        assert_eq!(
            extract_root_dir(root).unwrap(),
            root.join("node-v24.17.0-darwin-arm64")
        );
        std::fs::create_dir(root.join("second")).unwrap();
        assert!(extract_root_dir(root).is_err()); // two dirs
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_creates_and_prunes_tool_shims() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let yerd_bin = tmp.path().join("yerd");
        std::fs::write(&yerd_bin, b"#!fake").unwrap();
        let bin = bin_dir(&dirs);

        // Install node (versioned subdir) + composer (phar), leave bun absent.
        let node_root = tool_dir(&dirs, Tool::Node).join("node-v24.17.0-darwin-arm64");
        std::fs::create_dir_all(node_root.join("bin")).unwrap();
        std::fs::write(node_root.join("bin").join("node"), b"n").unwrap();
        std::fs::write(node_root.join("bin").join("npm"), b"m").unwrap();
        std::fs::write(node_root.join("bin").join("npx"), b"x").unwrap();
        std::fs::write(tool_dir(&dirs, Tool::Node).join(VERSION_MARKER), "v24.17.0").unwrap();
        let composer_dir = tool_dir(&dirs, Tool::Composer);
        std::fs::create_dir_all(&composer_dir).unwrap();
        std::fs::write(composer_dir.join("composer.phar"), b"phar").unwrap();
        std::fs::write(composer_dir.join(VERSION_MARKER), "2.10.1").unwrap();

        // A stale bun shim from a prior install that's since been removed.
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(&yerd_bin, bin.join("bun")).unwrap();

        reconcile_tool_shims(&dirs, &yerd_bin).unwrap();

        // node/npm/npx → the dist bin; composer → the yerd binary.
        assert_eq!(
            std::fs::read_link(bin.join("node")).unwrap(),
            node_root.join("bin").join("node")
        );
        assert!(bin.join("npm").exists());
        assert!(bin.join("npx").exists());
        assert_eq!(std::fs::read_link(bin.join("composer")).unwrap(), yerd_bin);
        // The stale bun shim (tool not installed) was pruned.
        assert!(!bin.join("bun").exists());
    }

    #[test]
    fn stage_and_swap_places_and_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        stage_and_swap(&dirs, Tool::Bun, "bun-v1.0.0", |s| {
            std::fs::create_dir(s.join("bun-darwin-aarch64")).unwrap();
            std::fs::write(s.join("bun-darwin-aarch64").join("bun"), b"v1").unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(
            installed_version(&dirs, Tool::Bun).as_deref(),
            Some("bun-v1.0.0")
        );
        // Reinstall replaces in place — still exactly one child dir.
        stage_and_swap(&dirs, Tool::Bun, "bun-v1.1.0", |s| {
            std::fs::create_dir(s.join("bun-darwin-aarch64")).unwrap();
            std::fs::write(s.join("bun-darwin-aarch64").join("bun"), b"v2").unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(
            installed_version(&dirs, Tool::Bun).as_deref(),
            Some("bun-v1.1.0")
        );
        assert!(extract_root_dir(&tool_dir(&dirs, Tool::Bun)).is_ok());
    }
}
