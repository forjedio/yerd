//! `yerdd` lifecycle from the GUI: locate, install, start, stop.
//!
//! All host-side — the daemon may be down when these run. Mirrors `elevate.rs`:
//! resolve trusted binaries relative to our own exe, do blocking work off the
//! async runtime, and thread every failure through [`GuiError`] (the crate bans
//! `unwrap`/`expect`/`panic` under clippy). The OS service mechanism
//! (systemd/launchd) lives in [`crate::autostart`]; this module owns binary
//! resolution, the release download/install, and the start/stop orchestration.

use std::path::{Path, PathBuf};

use crate::error::GuiError;

/// The GitHub repo releases are published under (matches `scripts/install.sh`).
pub(crate) const REPO: &str = "forjedio/yerd";

// ── binary resolution ───────────────────────────────────────────────────────

/// `$HOME`, or `None` if unset.
pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Where we install downloaded binaries (sudo-free, on `PATH` for most shells).
pub(crate) fn install_dir() -> Result<PathBuf, GuiError> {
    let home = home_dir().ok_or_else(|| GuiError::internal("HOME is not set"))?;
    Ok(home.join(".local").join("bin"))
}

/// Directories searched for an installed binary, in priority order.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local").join("bin"));
    }
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/usr/bin"));
    dirs
}

/// Resolve an installed binary: first beside our own executable (Linux `.deb`
/// installs `yerd`/`yerdd`/`yerd-helper` as siblings of `yerd-gui` in
/// `/usr/bin`), then the usual install dirs. Mirrors
/// `bin/yerd/src/elevate.rs::sibling_binaries`.
pub(crate) fn resolve_binary(name: &str) -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    search_dirs()
        .into_iter()
        .map(|d| d.join(name))
        .find(|c| c.is_file())
}

/// The resolved `yerdd` path, if installed.
pub(crate) fn resolve_yerdd() -> Option<PathBuf> {
    resolve_binary("yerdd")
}

/// Is `yerdd` installed (a binary exists on disk)? Note: independent of whether
/// it's *running* — the auto-install flow gates on reachability too.
#[tauri::command]
pub fn daemon_installed() -> bool {
    resolve_yerdd().is_some()
}

// ── install (download a matching release) ────────────────────────────────────

/// The Rust target triple of the release asset for this host.
fn target_triple() -> Result<String, GuiError> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => {
            return Err(GuiError::internal(format!(
                "unsupported architecture: {other}"
            )))
        }
    };
    match std::env::consts::OS {
        // Asset label, not the rustc triple: releases use `generic-linux`.
        "linux" => Ok(format!("{arch}-generic-linux-gnu")),
        "macos" => {
            if arch != "aarch64" {
                return Err(GuiError::internal(
                    "Yerd ships Apple Silicon (arm64) builds only; install the CLI manually on Intel Macs (see the docs).",
                ));
            }
            Ok(format!("{arch}-apple-darwin"))
        }
        other => Err(GuiError::internal(format!(
            "auto-install is not supported on this platform ({other}); install the CLI manually"
        ))),
    }
}

fn http_client() -> Result<reqwest::Client, GuiError> {
    reqwest::Client::builder()
        .user_agent(concat!("yerd-gui/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| GuiError::internal(format!("could not build HTTP client: {e}")))
}

/// Resolve the version to install: the GUI's own version if a release is
/// published for it, else the latest stable release (mirrors install.sh, which
/// uses `releases/latest` to avoid 404s on dev builds whose version isn't tagged).
async fn resolve_version(client: &reqwest::Client) -> Result<String, GuiError> {
    let pkg = env!("CARGO_PKG_VERSION");
    let sums = format!("https://github.com/{REPO}/releases/download/v{pkg}/SHA256SUMS");
    if let Ok(resp) = client.head(&sums).send().await {
        if resp.status().is_success() {
            return Ok(pkg.to_owned());
        }
    }
    // Fall back to the latest stable release. (Parse bytes ourselves — reqwest's
    // `.json()` needs the `json` feature, which our minimal build omits.)
    let api = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = http_bytes(client, &api).await?;
    let json: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| GuiError::internal(format!("bad release JSON: {e}")))?;
    let tag = json
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| GuiError::internal("latest release has no tag_name"))?;
    Ok(tag.trim_start_matches('v').to_owned())
}

async fn http_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, GuiError> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| GuiError::internal(format!("download failed ({url}): {e}")))?
        .error_for_status()
        .map_err(|e| GuiError::internal(format!("download failed ({url}): {e}")))?;
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| GuiError::internal(format!("read failed ({url}): {e}")))
}

/// Verify `bytes` against the `<sha256>  <name>` line for `asset` in a
/// `SHA256SUMS` body (exact filename match, like install.sh).
fn verify_sha256(sums: &str, asset: &str, bytes: &[u8]) -> Result<(), GuiError> {
    use sha2::{Digest, Sha256};
    let expected = sums
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let name = parts.next()?.trim_start_matches('*'); // sha256sum binary-mode "*"
            (name == asset).then(|| hash.to_owned())
        })
        .ok_or_else(|| GuiError::internal(format!("no checksum listed for {asset}")))?;
    let actual = hex::encode(Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(&expected) {
        Ok(())
    } else {
        Err(GuiError::internal(format!("checksum mismatch for {asset}")))
    }
}

/// Extract `yerd`/`yerdd`/`yerd-helper` (top-level entries only) from a
/// `.tar.gz` into `dest` at mode `0755`, atomically per file. Returns the
/// installed paths. Blocking (caller runs it on `spawn_blocking`).
fn extract_binaries(tar_gz: &[u8], dest: &Path) -> Result<Vec<PathBuf>, GuiError> {
    use flate2::read::GzDecoder;
    use std::io::Read as _;

    std::fs::create_dir_all(dest)
        .map_err(|e| GuiError::internal(format!("could not create {}: {e}", dest.display())))?;

    let wanted = ["yerd", "yerdd", "yerd-helper"];
    let mut archive = tar::Archive::new(GzDecoder::new(tar_gz));
    let mut installed = Vec::new();
    let entries = archive
        .entries()
        .map_err(|e| GuiError::internal(format!("could not read tarball: {e}")))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| GuiError::internal(format!("bad tar entry: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| GuiError::internal(format!("bad tar path: {e}")))?
            .into_owned();
        // Top-level binary only (`yerd`, not `nested/yerd`).
        let is_binary = path.components().count() == 1
            && path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| wanted.contains(&n));
        if !is_binary {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| GuiError::internal("tar entry has no name"))?;
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| GuiError::internal(format!("could not read {name} from tarball: {e}")))?;
        let out = dest.join(name);
        write_executable(&out, &buf)?;
        installed.push(out);
    }
    if installed.len() != wanted.len() {
        return Err(GuiError::internal(format!(
            "release tarball was missing one of {wanted:?} (found {} of {})",
            installed.len(),
            wanted.len()
        )));
    }
    Ok(installed)
}

/// Write `bytes` to `path` at mode `0755` via a temp file + rename (atomic).
fn write_executable(path: &Path, bytes: &[u8]) -> Result<(), GuiError> {
    use std::io::Write as _;
    let parent = path
        .parent()
        .ok_or_else(|| GuiError::internal("install path has no parent"))?;
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| GuiError::internal("install path has no file name"))?;
    let tmp = parent.join(format!(".{file_name}.tmp"));
    {
        let mut f = std::fs::File::create(&tmp)
            .map_err(|e| GuiError::internal(format!("could not create {}: {e}", tmp.display())))?;
        f.write_all(bytes)
            .map_err(|e| GuiError::internal(format!("could not write {}: {e}", tmp.display())))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| GuiError::internal(format!("could not chmod {}: {e}", tmp.display())))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        GuiError::internal(format!(
            "could not move {} -> {}: {e}",
            tmp.display(),
            path.display()
        ))
    })
}

/// macOS: ad-hoc sign a binary so AMFI lets it exec on Apple Silicon. Older /
/// unsigned releases ship unsigned Mach-Os; on arm64 an unsigned binary is
/// SIGKILLed ("Killed: 9") regardless of quarantine. This is the fallback used
/// when [`verify_signed`] finds no valid signature.
#[cfg(target_os = "macos")]
fn adhoc_sign(path: &Path) -> Result<(), GuiError> {
    let status = std::process::Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(path)
        .status()
        .map_err(|e| {
            GuiError::internal(format!(
                "codesign unavailable ({e}); an unsigned binary won't run on Apple Silicon — install the CLI manually"
            ))
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(GuiError::internal(format!(
            "codesign failed for {}",
            path.display()
        )))
    }
}

/// macOS: does this binary already carry a *valid* code signature? `codesign
/// --verify --strict` is a local, offline check (no `--deep`, no `spctl`, no
/// notarisation lookup), so it works on an air-gapped install. It accepts any
/// valid signature — a Developer-ID release binary, or one we ad-hoc signed on
/// a previous install — and we skip re-signing those, which keeps a notarised
/// Developer-ID signature intact. It returns false only when there is no valid
/// signature (an unsigned legacy binary), which then needs ad-hoc signing to
/// exec on arm64. (A bad download is caught upstream by `verify_sha256`.)
#[cfg(target_os = "macos")]
fn verify_signed(path: &Path) -> bool {
    std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(path)
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Blocking install: extract, then on macOS make sure each binary has a valid
/// signature — preserving any existing one (a Developer-ID/notarised release, or
/// a prior ad-hoc signature) and ad-hoc signing only a binary with no valid
/// signature, so AMFI lets it exec on arm64.
fn install_blocking(tar_gz: &[u8], dest: &Path) -> Result<(), GuiError> {
    let installed = extract_binaries(tar_gz, dest)?;
    #[cfg(target_os = "macos")]
    for p in &installed {
        if !verify_signed(p) {
            adhoc_sign(p)?;
        }
    }
    let _ = &installed;
    Ok(())
}

fn emit_progress(app: &tauri::AppHandle, message: &str) {
    use tauri::Emitter as _;
    let _ = app.emit("install-progress", message.to_owned());
}

/// Download the matching release and install `yerd`/`yerdd`/`yerd-helper` to
/// `~/.local/bin`. Progress is emitted as `install-progress` events.
#[tauri::command]
pub async fn install_daemon(app: tauri::AppHandle) -> Result<(), GuiError> {
    let triple = target_triple()?;
    let client = http_client()?;

    emit_progress(&app, "Resolving the latest Yerd release…");
    let version = resolve_version(&client).await?;
    let base = format!("https://github.com/{REPO}/releases/download/v{version}");

    emit_progress(&app, "Downloading checksums…");
    let sums_bytes = http_bytes(&client, &format!("{base}/SHA256SUMS")).await?;
    let sums = String::from_utf8(sums_bytes)
        .map_err(|e| GuiError::internal(format!("SHA256SUMS is not UTF-8: {e}")))?;

    let asset = format!("yerd-{version}-{triple}.tar.gz");
    emit_progress(&app, &format!("Downloading {asset}…"));
    let tarball = http_bytes(&client, &format!("{base}/{asset}")).await?;

    emit_progress(&app, "Verifying download…");
    verify_sha256(&sums, &asset, &tarball)?;

    emit_progress(&app, "Installing yerdd…");
    let dest = install_dir()?;
    tokio::task::spawn_blocking(move || install_blocking(&tarball, &dest))
        .await
        .map_err(|e| GuiError::internal(format!("install task failed: {e}")))??;

    emit_progress(&app, "Done");
    Ok(())
}

// ── start / stop ─────────────────────────────────────────────────────────────

/// Start the daemon. Prefers the per-user service (the single supervisor when
/// available); falls back to a detached `yerdd serve` only when no service
/// manager exists (in which case daemon-at-login is disabled in the UI). The
/// blocking service call runs off the async worker so the tray/UI never stalls.
pub(crate) async fn start() -> Result<(), GuiError> {
    tokio::task::spawn_blocking(crate::autostart::daemon_start)
        .await
        .map_err(|e| GuiError::internal(format!("start task failed: {e}")))?
}

/// Stop the daemon: via the service when one manages it, with a universal
/// SIGTERM-of-the-reported-pid fallback (covers `yerdd serve &`,
/// `cargo run -p yerdd`, etc.). The daemon shuts down gracefully on SIGTERM.
pub(crate) async fn stop() -> Result<(), GuiError> {
    let _ = tokio::task::spawn_blocking(crate::autostart::daemon_stop).await;
    if let Some(pid) = running_pid().await {
        sigterm(pid);
    }
    Ok(())
}

#[tauri::command]
pub async fn start_daemon() -> Result<(), GuiError> {
    start().await
}

#[tauri::command]
pub async fn stop_daemon() -> Result<(), GuiError> {
    stop().await
}

/// The running daemon's pid via a `status` IPC, or `None` if unreachable.
async fn running_pid() -> Option<u32> {
    match crate::ipc::exchange(&yerd_ipc::Request::Status).await {
        Ok(yerd_ipc::Response::Status { report }) => Some(report.daemon_pid),
        _ => None,
    }
}

/// Send SIGTERM to `pid` (best-effort; an already-dead pid is fine).
fn sigterm(pid: u32) {
    if let Ok(pid) = i32::try_from(pid) {
        // SAFETY: `kill` is a libc syscall with no memory effects; sending
        // SIGTERM to a pid cannot invoke UB. A stale pid just returns ESRCH.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
}

/// Spawn `yerdd serve` detached so it survives the GUI exiting (its own
/// session, stdio to /dev/null). Used only on the no-service-manager path
/// (Linux without systemd `--user`; macOS always has launchd).
#[cfg(target_os = "linux")]
pub(crate) fn spawn_detached() -> Result<(), GuiError> {
    let yerdd = resolve_yerdd().ok_or_else(|| GuiError::internal("yerdd is not installed"))?;
    let mut cmd = std::process::Command::new(&yerdd);
    cmd.arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: `setsid` in the child (pre-exec) detaches it into its own
        // session so it outlives the GUI; it touches no parent memory.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| GuiError::internal(format!("could not start {}: {e}", yerdd.display())))
}

// `verify_signed` is macOS-only (it gates the ad-hoc-sign skip), so the test
// only compiles/runs on the macOS runner.
#[cfg(all(test, target_os = "macos"))]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn verify_signed_accepts_signed_rejects_unsigned() {
        let dir = std::env::temp_dir().join(format!("yerd-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // A system binary carries a valid signature; a byte-verbatim copy keeps
        // it (exactly what tar extraction does to a Developer-ID release binary),
        // so verify_signed must accept it → the install skips ad-hoc signing.
        let signed = dir.join("ls");
        std::fs::copy("/bin/ls", &signed).unwrap();
        assert!(verify_signed(&signed), "copied signed binary should verify");

        // A copied real binary, stripped to unsigned, must fail verification →
        // the install ad-hoc signs it; and after ad-hoc signing it must verify
        // (the install's skip-on-reinstall invariant).
        let fresh = dir.join("yerdd");
        std::fs::copy("/bin/ls", &fresh).unwrap();
        std::process::Command::new("/usr/bin/codesign")
            .args(["--remove-signature"])
            .arg(&fresh)
            .status()
            .unwrap();
        assert!(!verify_signed(&fresh), "unsigned binary should not verify");
        adhoc_sign(&fresh).unwrap();
        assert!(verify_signed(&fresh), "ad-hoc signed binary should verify");

        // A plain, non-Mach-O file must also fail verification.
        let plain = dir.join("plain");
        std::fs::File::create(&plain)
            .unwrap()
            .write_all(b"not a signed binary")
            .unwrap();
        assert!(!verify_signed(&plain), "plain file should not verify");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
