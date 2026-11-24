//! Dev-tool installer subsystem - Composer, Node (node/npm/npx), Bun (bun/bunx),
//! pnpm (pnpm/pnpx), the Laravel installer, WP-CLI, and cloudflared (the
//! Cloudflare Tunnel connector Share uses).
//!
//! Most tools ship as a self-contained, relocatable binary (no global install):
//! Node's tarball, Bun's zip, pnpm's tarball, Composer's phar. yerd downloads + sha256-verifies
//! the latest release into `{data}/tools/<id>/` and symlinks the commands it
//! provides into `{data}/bin` (on `PATH` via `yerd path`). Same I/O-edge pattern
//! as `php_install`/`ext_install`: a `Downloader` trait is injected; the pure
//! resolution bits are inline + unit-tested. The Laravel installer and WP-CLI
//! are instead Composer packages, built via the managed Composer (see
//! `laravel`/`wp_cli`).

pub mod bun;
pub mod cloudflared;
pub mod composer;
pub mod external;
pub mod laravel;
pub mod node;
pub mod pnpm;
pub mod wp_cli;

use std::path::{Path, PathBuf};

use yerd_ipc::ToolStatus;
use yerd_php::Downloader;
use yerd_platform::PlatformDirs;

use crate::ext_install::sha256_hex;

/// The dev tools yerd can install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Composer (PHP dependency manager) - a phar run via the managed PHP.
    Composer,
    /// Node.js - `node`, `npm`, `npx`.
    Node,
    /// Bun - `bun`, `bunx`.
    Bun,
    /// pnpm - `pnpm`, `pnpx`.
    Pnpm,
    /// The Laravel installer (`laravel new`) - a Composer package run via the
    /// managed PHP, exposed as the `laravel` multi-call shim.
    Laravel,
    /// WP-CLI (`wp`) - the WordPress command-line tool, a Composer package
    /// (`wp-cli/wp-cli-bundle`) run via the managed PHP, exposed as the `wp`
    /// multi-call shim.
    WpCli,
    /// cloudflared - the Cloudflare Tunnel connector used by Share, a single
    /// static binary from Cloudflare's GitHub releases.
    Cloudflared,
}

/// Filename of the installed-version marker inside a tool's dir.
const VERSION_MARKER: &str = ".version";

impl Tool {
    /// Every tool, for `list_status` / reconcile.
    pub const ALL: [Tool; 7] = [
        Tool::Composer,
        Tool::Node,
        Tool::Bun,
        Tool::Pnpm,
        Tool::Laravel,
        Tool::WpCli,
        Tool::Cloudflared,
    ];

    /// Stable id used on the wire and as the on-disk dir name.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Tool::Composer => "composer",
            Tool::Node => "node",
            Tool::Bun => "bun",
            Tool::Pnpm => "pnpm",
            Tool::Laravel => "laravel",
            Tool::WpCli => "wp-cli",
            Tool::Cloudflared => "cloudflared",
        }
    }

    /// Human-readable name for the UI.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Tool::Composer => "Composer",
            Tool::Node => "Node.js",
            Tool::Bun => "Bun",
            Tool::Pnpm => "pnpm",
            Tool::Laravel => "Laravel Installer",
            Tool::WpCli => "WP-CLI",
            Tool::Cloudflared => "cloudflared",
        }
    }

    /// The command used to detect an *external* install of this tool on the
    /// user's PATH (the canonical entry point, not every exposed bin).
    #[must_use]
    pub const fn primary_bin(self) -> &'static str {
        match self {
            Tool::Composer => "composer",
            Tool::Node => "node",
            Tool::Bun => "bun",
            Tool::Pnpm => "pnpm",
            Tool::Laravel => "laravel",
            Tool::WpCli => "wp",
            Tool::Cloudflared => "cloudflared",
        }
    }

    /// The commands this tool exposes in `{data}/bin`.
    #[must_use]
    pub const fn exposed_bins(self) -> &'static [&'static str] {
        match self {
            Tool::Composer => &["composer"],
            Tool::Node => &["node", "npm", "npx"],
            Tool::Bun => &["bun", "bunx"],
            Tool::Pnpm => &["pnpm", "pnpx"],
            Tool::Laravel => &["laravel"],
            Tool::WpCli => &["wp"],
            Tool::Cloudflared => &["cloudflared"],
        }
    }

    /// Whether a copy of [`Self::primary_bin`] already on the user's PATH
    /// satisfies this tool's prerequisite, so a site-create job can use it and
    /// Tooling tags it `external`. This only decides whether a *managed* copy is
    /// required: Tooling still offers **Install** either way, so yerd's own copy
    /// can always be added alongside an external one.
    ///
    /// False for WP-CLI: every yerd path that runs `wp` (WordPress site
    /// creation, the admin-user list, URL sync) execs the *managed*
    /// `boot-fs.php` entry point directly rather than a PATH-resolved `wp`, so
    /// an external copy satisfies none of them and must not pass preflight. When
    /// one wrongly did, the WordPress wizard called the toolchain ready and a
    /// site create reached `wp core download`, then failed spawning a
    /// `boot-fs.php` that was never installed (issue #150).
    #[must_use]
    pub const fn accepts_external(self) -> bool {
        match self {
            Tool::Composer
            | Tool::Node
            | Tool::Bun
            | Tool::Pnpm
            | Tool::Laravel
            | Tool::Cloudflared => true,
            Tool::WpCli => false,
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
///
/// Node has no single marker: `node_default` is the daemon's configured
/// `[node] default`, falling back to a legacy single-version install.
pub(crate) fn installed_version(
    dirs: &PlatformDirs,
    tool: Tool,
    node_default: Option<&str>,
) -> Option<String> {
    if tool == Tool::Node {
        return yerd_platform::node::default_selector(dirs, node_default).and_then(|v| {
            yerd_platform::node::resolve(dirs, &v)
                .ok()
                .map(|v| format!("v{}", v.version))
        });
    }
    let v = std::fs::read_to_string(tool_dir(dirs, tool).join(VERSION_MARKER)).ok()?;
    let v = v.trim().to_owned();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Status of one tool (installed + version + the commands it provides).
/// `node_default` is the daemon's configured `[node] default`.
#[must_use]
pub fn status(dirs: &PlatformDirs, tool: Tool, node_default: Option<&str>) -> ToolStatus {
    let version = installed_version(dirs, tool, node_default);
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
        external: false,
        external_path: None,
        update_available: None,
    }
}

/// Status of every tool, for `Response::Tools`. Pure fs reads (no network/lock).
#[must_use]
pub fn list_status(dirs: &PlatformDirs, node_default: Option<&str>) -> Vec<ToolStatus> {
    Tool::ALL
        .iter()
        .map(|&t| status(dirs, t, node_default))
        .collect()
}

/// `latest` when it is a stable release strictly newer than `installed`, for
/// `ToolStatus::update_available`. Both sides may carry a `v` or `bun-v`
/// prefix; anything that is not semver yields `None`.
pub(crate) fn newer_version(installed: &str, latest: &str) -> Option<String> {
    let parse = |v: &str| yerd_update::parse_tag(v.strip_prefix("bun-").unwrap_or(v));
    let current = parse(installed)?;
    let candidate = parse(latest)?;
    (candidate.pre.is_empty() && candidate > current).then(|| latest.to_owned())
}

/// The highest stable `version` of `package` in a Packagist `p2/<package>.json`
/// document. Order is not assumed; pre-releases and non-semver entries such as
/// `dev-main` are skipped.
pub(crate) fn packagist_latest(bytes: &[u8], package: &str) -> Option<String> {
    let doc: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    doc.get("packages")?
        .get(package)?
        .as_array()?
        .iter()
        .filter_map(|entry| entry.get("version")?.as_str())
        .filter_map(|raw| {
            let parsed = yerd_update::parse_tag(raw)?;
            parsed.pre.is_empty().then_some((parsed, raw))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, raw)| raw.to_owned())
}

/// Fetch a Packagist `p2` document and pick its latest stable version.
pub(crate) async fn fetch_packagist_latest(
    dl: &dyn Downloader,
    package: &str,
) -> Result<String, ToolError> {
    let url = format!("https://repo.packagist.org/p2/{package}.json");
    let bytes = dl
        .download(&url)
        .await
        .map_err(|e| ToolError::Download(format!("packagist {package}: {e}")))?;
    packagist_latest(&bytes, package)
        .ok_or_else(|| ToolError::Download(format!("packagist {package}: no stable version")))
}

/// The latest published version of `tool`, for the notify-only update poll.
/// `Ok(None)` for Node, whose updates `node_updates` already tracks. Installs
/// nothing.
pub(crate) async fn latest(
    tool: Tool,
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
) -> Result<Option<String>, ToolError> {
    let version = match tool {
        Tool::Node => return Ok(None),
        Tool::Composer => composer::latest(dirs, dl).await?,
        Tool::Bun => bun::latest(dl).await?,
        Tool::Pnpm => pnpm::latest(dl).await?,
        Tool::Laravel => laravel::latest(dl).await?,
        Tool::WpCli => wp_cli::latest(dl).await?,
        Tool::Cloudflared => cloudflared::latest(dl).await?,
    };
    Ok(Some(version))
}

/// The installed version to compare against the latest one. The Laravel
/// installer's marker is always `installed`, so its real version comes from
/// Composer's `installed.php`; every other tool uses its marker.
pub(crate) fn comparable_version(
    dirs: &PlatformDirs,
    tool: Tool,
    marker: Option<&str>,
) -> Option<String> {
    match tool {
        Tool::Laravel => laravel::installed_installer_version(dirs),
        _ => marker.map(str::to_owned),
    }
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
/// live Composer output) is streamed to it. `proxy_env` is applied to the
/// Composer subprocess that builds the Laravel installer and WP-CLI.
pub async fn install(
    tool: Tool,
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    proxy_env: &yerd_core::network::ChildEnvPlan,
    progress: Option<&ProgressTx>,
) -> Result<(), ToolError> {
    note(progress, format!("Installing {}…", tool.display_name()));
    let result = match tool {
        Tool::Composer => composer::install(dirs, dl).await,
        Tool::Node => node::install(dirs, dl).await,
        Tool::Bun => bun::install(dirs, dl).await,
        Tool::Pnpm => pnpm::install(dirs, dl).await,
        Tool::Laravel => laravel::install(dirs, proxy_env, progress).await,
        Tool::WpCli => wp_cli::install(dirs, proxy_env, progress).await,
        Tool::Cloudflared => cloudflared::install(dirs, dl, progress).await,
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
    if tool == Tool::Node {
        let versions = yerd_platform::node::versions_dir(dirs);
        if versions.exists() {
            std::fs::remove_dir_all(&versions).map_err(|e| ToolError::Io(e.to_string()))?;
        }
    }
    let d = tool_dir(dirs, tool);
    if d.exists() {
        std::fs::remove_dir_all(&d).map_err(|e| ToolError::Io(format!("{}: {e}", d.display())))?;
    }
    Ok(())
}

/// Point every resolvable command of a Node, pnpm, or Bun install at the yerd
/// multi-call binary. The module-level `shim_links` still decides *which*
/// names get a link (none when the root or the Node default can't be
/// resolved); the CLI's `tool_shim` then resolves the same target at run time
/// and adds the outbound proxy environment before `exec`.
#[cfg(unix)]
fn via_yerd(links: Vec<(String, PathBuf)>, yerd_bin: &Path) -> Vec<(String, PathBuf)> {
    links
        .into_iter()
        .map(|(name, _)| (name, yerd_bin.to_path_buf()))
        .collect()
}

/// The `(name_in_bin, symlink_target)` pairs for an installed `tool`. Empty if
/// the tool isn't installed or its root can't be resolved. Every command is a
/// link to the yerd multi-call binary except `cloudflared`, which links straight
/// to the managed binary: its path carries no version to resolve, and it is the
/// documented outbound-proxy exception, so there is no environment to inject.
#[cfg(unix)]
fn shim_links(
    dirs: &PlatformDirs,
    tool: Tool,
    yerd_bin: &Path,
    node_default: Option<&str>,
) -> Vec<(String, PathBuf)> {
    match tool {
        Tool::Composer => vec![("composer".to_owned(), yerd_bin.to_path_buf())],
        Tool::Node => via_yerd(node::shim_links(dirs, node_default), yerd_bin),
        Tool::Bun => via_yerd(bun::shim_links(dirs), yerd_bin),
        Tool::Pnpm => via_yerd(pnpm::shim_links(dirs), yerd_bin),
        Tool::Laravel => vec![("laravel".to_owned(), yerd_bin.to_path_buf())],
        Tool::WpCli => vec![("wp".to_owned(), yerd_bin.to_path_buf())],
        Tool::Cloudflared => cloudflared_links(dirs),
    }
}

#[cfg(unix)]
fn cloudflared_links(dirs: &PlatformDirs) -> Vec<(String, PathBuf)> {
    vec![("cloudflared".to_owned(), cloudflared::binary_path(dirs))]
}

/// Create `tool`'s links in `bin` when it is installed, else prune its names
/// (symlinks only).
#[cfg(unix)]
fn reconcile_tool(
    dirs: &PlatformDirs,
    bin: &Path,
    tool: Tool,
    node_default: Option<&str>,
    links: impl FnOnce() -> Vec<(String, PathBuf)>,
) -> Result<(), ToolError> {
    if installed_version(dirs, tool, node_default).is_some() {
        for (name, target) in links() {
            crate::php_install::place_symlink(&bin.join(&name), &target)
                .map_err(|e| ToolError::Io(e.to_string()))?;
        }
    } else {
        for &name in tool.exposed_bins() {
            let p = bin.join(name);
            let is_link = std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink());
            if is_link {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    Ok(())
}

/// Reconcile only the `{data}/bin/cloudflared` link, which targets the managed
/// binary rather than the `yerd` multi-call binary, so it is kept even when the
/// `yerd` sibling cannot be located. Callers must hold the shared
/// `shim_reconcile` mutex. Unix-only; no-op elsewhere.
#[cfg(unix)]
pub fn reconcile_cloudflared_shim(dirs: &PlatformDirs) -> Result<(), ToolError> {
    let bin = bin_dir(dirs);
    std::fs::create_dir_all(&bin).map_err(|e| ToolError::Io(format!("{}: {e}", bin.display())))?;
    reconcile_tool(dirs, &bin, Tool::Cloudflared, None, || {
        cloudflared_links(dirs)
    })
}

#[cfg(not(unix))]
pub fn reconcile_cloudflared_shim(_dirs: &PlatformDirs) -> Result<(), ToolError> {
    Ok(())
}

/// Reconcile `{data}/bin` tool shims against what's installed: (re)create the
/// commands of installed tools, prune those of uninstalled tools. **Prunes by
/// name-ownership** (gated on `is_symlink`, never `target.exists()`), so a
/// dangling link after uninstall is still removed. Every tool is attempted even
/// when an earlier one fails; the first error is returned. Callers must hold the
/// shared `shim_reconcile` mutex (this writes the same dir as
/// `php_install::reconcile_shims`). Unix-only; no-op elsewhere.
#[cfg(unix)]
pub fn reconcile_tool_shims(
    dirs: &PlatformDirs,
    yerd_bin: &Path,
    node_default: Option<&str>,
) -> Result<(), ToolError> {
    let bin = bin_dir(dirs);
    std::fs::create_dir_all(&bin).map_err(|e| ToolError::Io(format!("{}: {e}", bin.display())))?;

    let mut first_err = None;
    for &tool in &Tool::ALL {
        let result = reconcile_tool(dirs, &bin, tool, node_default, || {
            shim_links(dirs, tool, yerd_bin, node_default)
        });
        if let Err(e) = result {
            first_err.get_or_insert(e);
        }
    }
    first_err.map_or(Ok(()), Err)
}

#[cfg(not(unix))]
pub fn reconcile_tool_shims(
    _dirs: &PlatformDirs,
    _yerd_bin: &Path,
    _node_default: Option<&str>,
) -> Result<(), ToolError> {
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
/// one directory entry exists - never reconstructed from a version string.
pub(crate) fn extract_root_dir(dir: &Path) -> Result<PathBuf, ToolError> {
    yerd_platform::single_top_level_dir(dir).map_err(|e| ToolError::Unpack(e.to_string()))
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
    stage_at(&tool_dir(dirs, tool), version, unpack)
}

/// Atomically replace an isolated tool or release directory.
pub(crate) fn stage_at(
    final_dir: &Path,
    version: &str,
    unpack: impl FnOnce(&Path) -> Result<(), ToolError>,
) -> Result<(), ToolError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);

    let tools_root = final_dir
        .parent()
        .ok_or_else(|| ToolError::Io("missing install parent".to_owned()))?;
    std::fs::create_dir_all(tools_root)
        .map_err(|e| ToolError::Io(format!("{}: {e}", tools_root.display())))?;
    let staging = tools_root.join(format!(
        ".staging-{}-{}-{seq}",
        final_dir.file_name().unwrap_or_default().to_string_lossy(),
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

    let backup = tools_root.join(format!(
        ".previous-{}-{}-{seq}",
        final_dir.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&backup);
    if final_dir.exists() {
        if let Err(e) = std::fs::rename(final_dir, &backup) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(ToolError::Io(format!("{}: {e}", final_dir.display())));
        }
    }
    if let Err(e) = std::fs::rename(&staging, final_dir) {
        if backup.exists() {
            let _ = std::fs::rename(&backup, final_dir);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(ToolError::Io(format!("{}: {e}", final_dir.display())));
    }
    let _ = std::fs::remove_dir_all(&backup);
    Ok(())
}

/// Stream a child pipe line-by-line into `progress` (no-op if either is
/// absent). Used by the Composer-package tools (Laravel installer, WP-CLI) to
/// forward `composer create-project` output.
pub(crate) async fn drain<R: tokio::io::AsyncRead + Unpin>(
    pipe: Option<R>,
    progress: Option<ProgressTx>,
) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let Some(pipe) = pipe else { return };
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(tx) = &progress {
            let _ = tx.send(line);
        }
    }
}

/// Move every entry of `from` into `to`. Both live under `{data}/tools`, so the
/// renames stay on one filesystem and are atomic + instant.
pub(crate) fn move_dir_contents(from: &Path, to: &Path) -> Result<(), ToolError> {
    let entries =
        std::fs::read_dir(from).map_err(|e| ToolError::Io(format!("{}: {e}", from.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| ToolError::Io(e.to_string()))?;
        let dest = to.join(entry.file_name());
        std::fs::rename(entry.path(), &dest)
            .map_err(|e| ToolError::Io(format!("{}: {e}", dest.display())))?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn newer_version_cases() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            ("v1.3.14", "bun-v1.3.15", Some("bun-v1.3.15")),
            ("v1.3.14", "v1.3.15", Some("v1.3.15")),
            ("2.8.1", "2.8.12", Some("2.8.12")),
            ("2.8.12", "2.8.12", None),
            ("2.8.12", "2.8.1", None),
            ("2.8.1", "2.9.0-RC1", None),
            ("v10.1.0", "v10.2.0", Some("v10.2.0")),
            ("garbage", "2.8.12", None),
            ("2.8.1", "garbage", None),
            ("installed", "v5.33.0", None),
            ("v5.32.0", "v5.33.0", Some("v5.33.0")),
            ("2024.6.1", "2024.12.0", Some("2024.12.0")),
            ("2024.12.0", "2024.12.0", None),
            ("2025.1.0", "2024.12.0", None),
            ("installed", "2025.11.1", None),
        ];
        for (installed, latest, want) in cases {
            assert_eq!(
                newer_version(installed, latest).as_deref(),
                *want,
                "{installed} vs {latest}"
            );
        }
    }

    const P2_INSTALLER: &str = r#"{"packages":{"laravel/installer":[
        {"name":"laravel/installer","version":"v5.33.0-beta.1","version_normalized":"5.33.0.0-beta1"},
        {"version":"v5.32.0","version_normalized":"5.32.0.0"},
        {"version":"v5.33.0","version_normalized":"5.33.0.0"},
        {"version":"dev-main"},
        {"version":"v4.5.1"}
    ]},"minified":"composer/2.0"}"#;

    #[test]
    fn packagist_latest_cases() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            (P2_INSTALLER, "laravel/installer", Some("v5.33.0")),
            (P2_INSTALLER, "wp-cli/wp-cli", None),
            (
                r#"{"packages":{"a/b":[{"version":"1.0.0-RC1"}]}}"#,
                "a/b",
                None,
            ),
            ("not json", "a/b", None),
            (r#"{"packages":{"a/b":{}}}"#, "a/b", None),
        ];
        for (doc, package, want) in cases {
            assert_eq!(
                packagist_latest(doc.as_bytes(), package).as_deref(),
                *want,
                "{package}"
            );
        }
    }

    #[tokio::test]
    async fn wp_cli_latest_reads_the_core_package_not_the_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = crate::test_support::dirs_in(tmp.path());
        let core = crate::test_support::UrlDl::default()
            .with(
                "https://repo.packagist.org/p2/wp-cli/wp-cli.json",
                r#"{"packages":{"wp-cli/wp-cli":[{"version":"v2.12.1"},{"version":"v2.12.0"}]}}"#,
            )
            .with(
                "https://repo.packagist.org/p2/wp-cli/wp-cli-bundle.json",
                r#"{"packages":{"wp-cli/wp-cli-bundle":[{"version":"v2.12.0"}]}}"#,
            );
        let got = latest(Tool::WpCli, &dirs, &core).await.unwrap();
        assert_eq!(got.as_deref(), Some("v2.12.1"));
        assert_eq!(
            newer_version("v2.12.0", "v2.12.1").as_deref(),
            Some("v2.12.1")
        );

        let bundle_only = crate::test_support::UrlDl::default().with(
            "https://repo.packagist.org/p2/wp-cli/wp-cli.json",
            r#"{"packages":{"wp-cli/wp-cli-bundle":[{"version":"v2.12.0"}]}}"#,
        );
        assert!(latest(Tool::WpCli, &dirs, &bundle_only).await.is_err());
    }

    #[tokio::test]
    async fn latest_per_tool_from_fixtures() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = crate::test_support::dirs_in(tmp.path());
        crate::test_support::fake_php(&dirs, "8.4");
        let dl = crate::test_support::UrlDl::default()
            .with(
                "https://getcomposer.org/versions",
                r#"{"stable":[{"path":"/download/2.8.12/composer.phar","version":"2.8.12","min-php":70205}]}"#,
            )
            .with(
                "https://api.github.com/repos/oven-sh/bun/releases/latest",
                r#"{"tag_name":"bun-v1.3.15"}"#,
            )
            .with(
                "https://api.github.com/repos/pnpm/pnpm/releases/latest",
                r#"{"tag_name":"v10.2.0","assets":[]}"#,
            )
            .with(
                "https://repo.packagist.org/p2/laravel/installer.json",
                P2_INSTALLER,
            )
            .with(
                "https://api.github.com/repos/cloudflare/cloudflared/releases/latest",
                r#"{"tag_name":"2025.11.1","assets":[]}"#,
            );
        let cases: &[(Tool, Option<&str>)] = &[
            (Tool::Composer, Some("2.8.12")),
            (Tool::Bun, Some("v1.3.15")),
            (Tool::Pnpm, Some("v10.2.0")),
            (Tool::Laravel, Some("v5.33.0")),
            (Tool::Cloudflared, Some("2025.11.1")),
            (Tool::Node, None),
        ];
        for (tool, want) in cases {
            assert_eq!(
                latest(*tool, &dirs, &dl).await.unwrap().as_deref(),
                *want,
                "{tool:?}"
            );
        }
        assert!(latest(Tool::WpCli, &dirs, &dl).await.is_err());
    }

    #[tokio::test]
    async fn composer_latest_needs_an_installed_php() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = crate::test_support::dirs_in(tmp.path());
        let dl = crate::test_support::UrlDl::default().with(
            "https://getcomposer.org/versions",
            r#"{"stable":[{"path":"/download/2.8.12/composer.phar","version":"2.8.12","min-php":70205}]}"#,
        );
        assert!(matches!(
            latest(Tool::Composer, &dirs, &dl).await,
            Err(ToolError::UnsupportedHost(_))
        ));
    }

    #[test]
    fn parse_and_ids_round_trip() {
        for t in Tool::ALL {
            assert_eq!(Tool::parse(t.id()), Some(t));
        }
        assert_eq!(Tool::parse("yarn"), None);
    }

    #[test]
    fn tool_metadata_is_consistent() {
        for t in Tool::ALL {
            assert!(!t.display_name().is_empty());
            assert!(!t.primary_bin().is_empty());
            assert!(
                t.exposed_bins().contains(&t.primary_bin()),
                "{t:?} primary_bin not in exposed_bins",
            );
        }
        assert_eq!(Tool::Node.exposed_bins(), &["node", "npm", "npx"]);
        assert_eq!(Tool::Bun.exposed_bins(), &["bun", "bunx"]);
        assert_eq!(Tool::Pnpm.exposed_bins(), &["pnpm", "pnpx"]);
        assert_eq!(Tool::Composer.primary_bin(), "composer");
        assert_eq!(Tool::Laravel.display_name(), "Laravel Installer");
    }

    /// Issue #150: an external `wp` can't stand in for the managed build, so
    /// Tooling must keep offering Install rather than reporting it external.
    #[test]
    fn wp_cli_never_accepts_an_external_copy() {
        assert!(!Tool::WpCli.accepts_external());
        for t in Tool::ALL {
            if t != Tool::WpCli {
                assert!(t.accepts_external(), "{t:?} should accept an external copy");
            }
        }
    }

    #[test]
    fn installed_version_ignores_blank_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let d = tool_dir(&dirs, Tool::Composer);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(VERSION_MARKER), "   \n").unwrap();
        assert_eq!(installed_version(&dirs, Tool::Composer, None), None);
        std::fs::write(d.join(VERSION_MARKER), "  2.10.1\n").unwrap();
        assert_eq!(
            installed_version(&dirs, Tool::Composer, None).as_deref(),
            Some("2.10.1")
        );
    }

    #[test]
    fn uninstall_removes_tool_dir_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let d = tool_dir(&dirs, Tool::Bun);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(VERSION_MARKER), "bun-v1.0.0").unwrap();
        uninstall(&dirs, Tool::Bun).unwrap();
        assert!(!d.exists());
        uninstall(&dirs, Tool::Bun).unwrap();
    }

    #[test]
    fn list_status_covers_every_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let all = list_status(&dirs, None);
        assert_eq!(all.len(), Tool::ALL.len());
        assert!(all.iter().all(|s| !s.installed && !s.external));
    }

    #[test]
    fn sha_for_asset_matches_exact_filename_among_decoys() {
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
        let s = status(&dirs, Tool::Node, None);
        assert!(!s.installed);
        assert_eq!(s.binaries, vec!["node", "npm", "npx"]);
        let d = tool_dir(&dirs, Tool::Node);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(VERSION_MARKER), "v24.17.0").unwrap();
        let bin = d.join("node-v24.17.0-test/bin");
        std::fs::create_dir_all(&bin).unwrap();
        for name in ["node", "npm", "npx"] {
            std::fs::write(bin.join(name), "fake").unwrap();
        }
        let s = status(&dirs, Tool::Node, None);
        assert!(s.installed);
        assert_eq!(s.version.as_deref(), Some("v24.17.0"));
    }

    #[test]
    fn node_status_uses_the_passed_default_not_the_config_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        for v in ["22.9.0", "24.2.0"] {
            let root = yerd_platform::node::versions_dir(&dirs).join(v);
            let bin = root.join("node-test/bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(root.join(VERSION_MARKER), v).unwrap();
            for name in ["node", "npm", "npx"] {
                std::fs::write(bin.join(name), "fake").unwrap();
            }
        }
        std::fs::create_dir_all(&dirs.config).unwrap();
        std::fs::write(
            dirs.config.join("yerd.toml"),
            "version = 27\n[node]\ndefault = \"24.2.0\"\n",
        )
        .unwrap();
        let s = status(&dirs, Tool::Node, Some("22.9.0"));
        assert_eq!(s.version.as_deref(), Some("v22.9.0"));
        assert!(status(&dirs, Tool::Node, None).version.is_none());
    }

    #[test]
    fn extract_root_dir_maps_errors_to_unpack() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(matches!(extract_root_dir(root), Err(ToolError::Unpack(_))));
        std::fs::create_dir(root.join("node-v24.17.0-darwin-arm64")).unwrap();
        assert_eq!(
            extract_root_dir(root).unwrap(),
            root.join("node-v24.17.0-darwin-arm64")
        );
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_creates_and_prunes_tool_shims() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let yerd_bin = tmp.path().join("yerd");
        std::fs::write(&yerd_bin, b"#!fake").unwrap();
        let bin = bin_dir(&dirs);

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

        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(&yerd_bin, bin.join("bun")).unwrap();

        reconcile_tool_shims(&dirs, &yerd_bin, None).unwrap();

        for name in ["node", "npm", "npx", "composer"] {
            assert_eq!(
                std::fs::read_link(bin.join(name)).unwrap(),
                yerd_bin,
                "{name}"
            );
        }
        assert!(node_root.join("bin").join("node").is_file());
        assert!(!bin.join("bun").exists());

        let bun_root = tool_dir(&dirs, Tool::Bun).join("bun-darwin-aarch64");
        std::fs::create_dir_all(&bun_root).unwrap();
        std::fs::write(bun_root.join("bun"), b"b").unwrap();
        std::fs::write(
            tool_dir(&dirs, Tool::Bun).join(VERSION_MARKER),
            "bun-v1.0.0",
        )
        .unwrap();
        reconcile_tool_shims(&dirs, &yerd_bin, None).unwrap();
        for name in ["bun", "bunx"] {
            assert_eq!(
                std::fs::read_link(bin.join(name)).unwrap(),
                yerd_bin,
                "{name}"
            );
        }

        cloudflared::install_binary_for_test(&dirs, "2025.11.1", b"#!/bin/sh\n");
        reconcile_tool_shims(&dirs, &yerd_bin, None).unwrap();
        assert_eq!(
            std::fs::read_link(bin.join("cloudflared")).unwrap(),
            cloudflared::binary_path(&dirs)
        );

        uninstall(&dirs, Tool::Cloudflared).unwrap();
        reconcile_tool_shims(&dirs, &yerd_bin, None).unwrap();
        assert!(std::fs::symlink_metadata(bin.join("cloudflared")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_tries_every_tool_when_one_link_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let yerd_bin = tmp.path().join("yerd");
        std::fs::write(&yerd_bin, b"#!fake").unwrap();
        let bin = bin_dir(&dirs);
        let composer_dir = tool_dir(&dirs, Tool::Composer);
        std::fs::create_dir_all(&composer_dir).unwrap();
        std::fs::write(composer_dir.join(VERSION_MARKER), "2.10.1").unwrap();
        std::fs::create_dir_all(bin.join("composer").join("blocker")).unwrap();
        cloudflared::install_binary_for_test(&dirs, "2025.11.1", b"#!/bin/sh\n");

        assert!(reconcile_tool_shims(&dirs, &yerd_bin, None).is_err());
        assert_eq!(
            std::fs::read_link(bin.join("cloudflared")).unwrap(),
            cloudflared::binary_path(&dirs)
        );
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_cloudflared_shim_links_then_prunes_without_yerd() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let link = bin_dir(&dirs).join("cloudflared");

        reconcile_cloudflared_shim(&dirs).unwrap();
        assert!(std::fs::symlink_metadata(&link).is_err());

        cloudflared::install_binary_for_test(&dirs, "2025.11.1", b"#!/bin/sh\n");
        reconcile_cloudflared_shim(&dirs).unwrap();
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            cloudflared::binary_path(&dirs)
        );

        uninstall(&dirs, Tool::Cloudflared).unwrap();
        reconcile_cloudflared_shim(&dirs).unwrap();
        assert!(std::fs::symlink_metadata(&link).is_err());
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
            installed_version(&dirs, Tool::Bun, None).as_deref(),
            Some("bun-v1.0.0")
        );
        stage_and_swap(&dirs, Tool::Bun, "bun-v1.1.0", |s| {
            std::fs::create_dir(s.join("bun-darwin-aarch64")).unwrap();
            std::fs::write(s.join("bun-darwin-aarch64").join("bun"), b"v2").unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(
            installed_version(&dirs, Tool::Bun, None).as_deref(),
            Some("bun-v1.1.0")
        );
        assert!(extract_root_dir(&tool_dir(&dirs, Tool::Bun)).is_ok());
    }

    #[test]
    fn move_dir_contents_moves_all_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("from");
        let to = tmp.path().join("to");
        std::fs::create_dir_all(from.join("vendor")).unwrap();
        std::fs::write(from.join("composer.json"), b"{}").unwrap();
        std::fs::create_dir_all(&to).unwrap();
        move_dir_contents(&from, &to).unwrap();
        assert!(to.join("vendor").is_dir());
        assert!(to.join("composer.json").is_file());
    }
}
