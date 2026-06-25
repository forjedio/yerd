//! Detect dev tools installed *outside* Yerd (on the user's PATH) so the Tooling
//! page can show them as "External" and the Laravel scaffold can use them.
//!
//! The daemon runs under launchd / `systemd --user` with a **restricted** PATH,
//! so it can't see Homebrew / fnm / global-Composer tools from its own env. We
//! resolve the user's **interactive-login** shell PATH to find them. Spawning the
//! shell is the I/O edge; the path-walking is pure. Unix-only — Windows yields
//! `None`/no externals.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::Tool;

/// Markers wrapping the printed PATH so rc-file banners / `echo` can't corrupt
/// the capture — we extract strictly between them.
const BEGIN: &str = "__YERD_PATH_BEGIN__";
const END: &str = "__YERD_PATH_END__";

/// How long a resolved PATH stays cached. `ListTools` can fire on each Tooling
/// page visit and spawning a heavy interactive-login shell every time is wasteful;
/// external installs rarely move, so a short TTL is plenty.
const PATH_TTL: Duration = Duration::from_secs(60);

/// `(resolved_at, dirs)` guarded for the process-wide PATH cache.
type PathCache = Mutex<Option<(Instant, Vec<PathBuf>)>>;

fn path_cache() -> &'static PathCache {
    static CACHE: OnceLock<PathCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Resolve the user's real PATH directories by running their interactive-login
/// shell (cached for [`PATH_TTL`]). `None` on non-Unix, spawn/timeout failure, or
/// unparseable output.
pub async fn resolve_user_path() -> Option<Vec<PathBuf>> {
    // Serve a recent cached result without spawning a shell.
    if let Ok(guard) = path_cache().lock() {
        if let Some((at, dirs)) = guard.as_ref() {
            if at.elapsed() < PATH_TTL {
                return Some(dirs.clone());
            }
        }
    }
    let raw = capture_path_string().await?;
    let inner = between(&raw, BEGIN, END)?;
    let dirs: Vec<PathBuf> = inner
        .split(':')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();
    if dirs.is_empty() {
        return None;
    }
    if let Ok(mut guard) = path_cache().lock() {
        *guard = Some((Instant::now(), dirs.clone()));
    }
    Some(dirs)
}

/// Find an executable named `bin` on `dirs`, skipping `exclude_dir` (Yerd's
/// `{data}/bin` shim dir) and rejecting any hit that canonicalises under
/// `data_root` (e.g. a user symlink into `{data}` — that's managed, not external).
#[must_use]
pub fn find_in_path(
    dirs: &[PathBuf],
    bin: &str,
    exclude_dir: &Path,
    data_root: &Path,
) -> Option<PathBuf> {
    // Canonicalise the data root too: on macOS `/var`→`/private/var` (and similar)
    // mean the candidate's canonical path won't `starts_with` an un-canonical root.
    let data_canon = std::fs::canonicalize(data_root).unwrap_or_else(|_| data_root.to_path_buf());
    for dir in dirs {
        if dir == exclude_dir {
            continue;
        }
        let cand = dir.join(bin);
        if !is_executable(&cand) {
            continue;
        }
        let canon = std::fs::canonicalize(&cand).unwrap_or_else(|_| cand.clone());
        if canon.starts_with(&data_canon) {
            continue; // a Yerd-managed binary reached via a symlink elsewhere.
        }
        return Some(cand);
    }
    None
}

/// The external install path of `tool`, if its primary command is on `dirs` and
/// not Yerd-managed.
#[must_use]
pub fn external_tool(
    dirs: &[PathBuf],
    tool: Tool,
    data_bin: &Path,
    data_root: &Path,
) -> Option<PathBuf> {
    find_in_path(dirs, tool.primary_bin(), data_bin, data_root)
}

/// Whether `p` is a regular file with any execute bit set.
#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    p.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Substring strictly between the first `begin` and the following `end`.
fn between<'a>(s: &'a str, begin: &str, end: &str) -> Option<&'a str> {
    let start = s.find(begin)? + begin.len();
    let rest = s.get(start..)?;
    let stop = rest.find(end)?;
    rest.get(..stop)
}

// ── shell spawn (Unix) ───────────────────────────────────────────────────────

#[cfg(unix)]
async fn capture_path_string() -> Option<String> {
    use std::process::Stdio;

    let shell = user_shell();
    let args = shell_invocation(&shell);

    let mut cmd = tokio::process::Command::new(&shell);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let child = cmd.spawn().ok()?;
    // Bound the wait — a misconfigured rc could hang; `kill_on_drop` reaps it.
    let out = tokio::time::timeout(std::time::Duration::from_secs(3), child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    // Don't require a zero exit: some shells exit non-zero from rc quirks, but
    // still print our marker-delimited PATH on stdout, which is all we need.
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(not(unix))]
async fn capture_path_string() -> Option<String> {
    None
}

/// The user's login shell: `$SHELL` → the passwd entry for this uid (launchd /
/// `systemd --user` often drop `$SHELL`) → a per-OS default.
#[cfg(unix)]
fn user_shell() -> String {
    if let Some(s) = std::env::var_os("SHELL") {
        if !s.is_empty() {
            return s.to_string_lossy().into_owned();
        }
    }
    if let Ok(Some(user)) = nix::unistd::User::from_uid(nix::unistd::getuid()) {
        if !user.shell.as_os_str().is_empty() {
            return user.shell.to_string_lossy().into_owned();
        }
    }
    if cfg!(target_os = "macos") {
        "/bin/zsh".to_owned()
    } else {
        "/bin/bash".to_owned()
    }
}

/// Build the shell args to print the PATH between [`BEGIN`]/[`END`] markers.
/// Interactive (`-i`) is load-bearing: fnm/nvm mutate PATH from `~/.zshrc` /
/// `~/.bashrc`, which a non-interactive login shell never sources. Login (`-l`)
/// additionally picks up profile-installed tools (e.g. Homebrew). `dash` rejects
/// `-l`, so the POSIX fallback is interactive-only.
#[cfg(unix)]
fn shell_invocation(shell: &str) -> Vec<String> {
    use yerd_platform::pure::shell_profile::{detect_shell, Shell};

    let base = Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let posix_cmd = format!("printf '{BEGIN}%s{END}' \"$PATH\"");
    match detect_shell(base) {
        Some(Shell::Fish) => vec![
            "-il".to_owned(),
            "-c".to_owned(),
            format!("printf '{BEGIN}%s{END}' (string join : $PATH)"),
        ],
        Some(Shell::Zsh | Shell::Bash) => vec!["-ilc".to_owned(), posix_cmd],
        // POSIX / unknown (incl. dash, which rejects `-l`): interactive only.
        _ => vec!["-ic".to_owned(), posix_cmd],
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn touch_exec(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn between_extracts_inner() {
        assert_eq!(
            between("noise__A__/usr/bin__B__tail", "__A__", "__B__"),
            Some("/usr/bin")
        );
        assert_eq!(between("no markers", "__A__", "__B__"), None);
    }

    #[test]
    fn find_in_path_skips_exclude_dir_and_data_root() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let data_bin = data.join("bin");
        let ext = tmp.path().join("opt");
        std::fs::create_dir_all(&data_bin).unwrap();
        std::fs::create_dir_all(&ext).unwrap();

        // A managed shim in {data}/bin and a genuine external in /opt.
        touch_exec(&data_bin, "composer");
        let real = touch_exec(&ext, "composer");

        let dirs = vec![data_bin.clone(), ext.clone()];
        // {data}/bin is excluded → resolves to the external one.
        let found = find_in_path(&dirs, "composer", &data_bin, &data).unwrap();
        assert_eq!(found, real);

        // Only the managed dir present → nothing external.
        assert!(find_in_path(
            std::slice::from_ref(&data_bin),
            "composer",
            &data_bin,
            &data
        )
        .is_none());
    }

    #[test]
    fn find_in_path_rejects_symlink_into_data() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let data_bin = data.join("bin");
        let userbin = tmp.path().join("userbin");
        std::fs::create_dir_all(&data_bin).unwrap();
        std::fs::create_dir_all(&userbin).unwrap();
        let managed = touch_exec(&data_bin, "node");
        // ~/bin/node -> {data}/bin/node : on PATH but canonicalises into {data}.
        std::os::unix::fs::symlink(&managed, userbin.join("node")).unwrap();

        // userbin is NOT the excluded dir, but the canonicalize guard rejects it.
        assert!(find_in_path(&[userbin], "node", &data_bin, &data).is_none());
    }
}
