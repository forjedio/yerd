//! Filesystem hardening helpers for daemon-owned paths.
//!
//! `yerd-platform`'s `PlatformDirs` contract makes the *caller* responsible
//! for locking down the runtime directory and the secrets it holds -
//! specifically because the Linux fallback when `XDG_RUNTIME_DIR` is unset is
//! the world-traversable `/tmp/yerd-$UID`. The daemon's only access control
//! over the IPC socket is the directory/socket permissions, so these helpers
//! enforce `0o700` on the runtime dir and `0o600` on the socket and on the CA
//! private key.
//!
//! On Windows the mode operations are no-ops - POSIX bits do not apply - and
//! the equivalent hardening is a DACL restricted to the current user's SID,
//! applied through `apply_dacl`. On any other non-Unix target both are no-ops;
//! the directory is still created.

use std::io;
use std::path::Path;

/// Create `path` (and parents) and, on Unix, force its mode to `0o700`. On
/// Windows, replace its DACL with a single inheritable full-control ACE for the
/// current user.
///
/// `create_dir_all` is idempotent; the subsequent `set_permissions` tightens
/// the mode whether the directory was just created (umask may have widened it)
/// or already existed. If a different user pre-created the directory, the
/// `chmod` fails with `PermissionDenied` and the daemon refuses to start
/// rather than trusting a directory it cannot lock down - fail-closed.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    apply_dacl(path, true);
    set_mode(path, 0o700)
}

/// On Unix, set `path`'s mode to `0o600` (owner read/write only). On Windows,
/// replace its DACL with a single full-control ACE for the current user. No-op
/// elsewhere. Used for the CA private key and the IPC socket.
pub fn restrict_to_owner(path: &Path) -> io::Result<()> {
    apply_dacl(path, false);
    set_mode(path, 0o600)
}

/// On Unix, set `path`'s mode to `0o644` (owner read/write, others read-only).
/// No-op elsewhere. Used for the **public** CA certificate: world-readable is
/// fine for a cert, but it must not be group/world-*writable* or the trust
/// helper refuses to install it (a tamper guard). Newly-created files inherit
/// the umask, which on common setups (`umask 002`) leaves `0o664` -
/// group-writable - so we force the mode explicitly.
///
/// Deliberately still a no-op on Windows, unlike its two siblings: it exists
/// only to strip group/world *write* from a file that must stay world-readable,
/// and there is no Windows analogue of that split. Narrowing the public CA
/// certificate to the owner's SID would be a different (and unwanted) change.
pub fn restrict_writes_to_owner(path: &Path) -> io::Result<()> {
    set_mode(path, 0o644)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// No-op on non-Unix: POSIX mode bits do not apply. Kept fallible to mirror the
/// Unix signature so callers stay platform-agnostic.
#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

/// The `icacls` argv that drops every ACE `path` carries, inherited or not, so
/// the [`icacls_args`] pass that follows starts from an empty DACL.
///
/// `/reset` cannot share an invocation with `/inheritance` or `/grant` (icacls
/// answers `Invalid parameter`), which is why this is a separate argv rather
/// than two more elements on the one below.
///
/// Pure and compiled on every OS, like its sibling.
#[cfg_attr(not(windows), allow(dead_code))]
fn icacls_reset_args(path: &Path) -> Vec<String> {
    vec![path.to_string_lossy().into_owned(), "/reset".to_owned()]
}

/// The `icacls` argv that reduces `path`'s DACL to one full-control ACE for
/// `sid`, dropping everything it would otherwise inherit.
///
/// `/inheritance:r` removes the inherited ACEs and `/grant:r` *replaces* the
/// principal's existing grants rather than adding to them. That pair alone is
/// not enough: neither option touches an **explicit** ACE belonging to another
/// principal, and a directory created under a parent with no inheritable ACEs
/// gets the creating token's default DACL - `SYSTEM`, `Administrators`, the
/// user - as explicit ACEs. Hence the [`icacls_reset_args`] pass first; this one
/// then writes the single ACE onto the empty DACL it leaves behind. A directory
/// takes `(OI)(CI)` so new children inherit it; a file takes a bare `F`. The `*`
/// prefix on the SID is required: without it icacls reads the string as an
/// account *name* and fails with "No mapping between account names and security
/// IDs was done".
///
/// Shelling out to `icacls.exe` is deliberate. The Win32 ACL APIs
/// (`SetNamedSecurityInfo` and friends) are `unsafe` FFI and the workspace
/// forbids `unsafe`, so this follows the same absolute-path-to-System32 trade
/// the repo already makes for `whoami.exe`
/// (`yerd_platform::current_user_sid`) and `tasklist.exe` (`yerd-service-ctl`).
///
/// Pure and compiled on every OS so the table tests run in CI everywhere.
#[cfg_attr(not(windows), allow(dead_code))]
fn icacls_args(path: &Path, sid: &str, recursive_inherit: bool) -> Vec<String> {
    let grant = if recursive_inherit {
        format!("*{sid}:(OI)(CI)F")
    } else {
        format!("*{sid}:F")
    };
    vec![
        path.to_string_lossy().into_owned(),
        "/inheritance:r".to_owned(),
        "/grant:r".to_owned(),
        grant,
    ]
}

/// Restrict `path`'s DACL to a single full-control ACE for the current user,
/// the Windows counterpart of the Unix `chmod`. `recursive_inherit` marks a
/// directory, whose ACE is made inheritable so its children are covered too.
///
/// Runs `icacls` twice: a `/reset` pass to clear the DACL, then the `/grant:r`
/// pass that writes the one ACE (see [`icacls_reset_args`] for why they cannot
/// be one call). Between the two the path carries whatever its parent hands
/// down, which is no wider than a path freshly created there would have had.
///
/// Spawned with `CREATE_NO_WINDOW` because the daemon itself runs without a
/// console: a console child would otherwise get one allocated and flash a
/// window, once per hardened path.
///
/// **A failure here is logged and swallowed, where the Unix `chmod` failure is
/// fatal.** That divergence is deliberate. The Unix rule fails closed because
/// the `XDG_RUNTIME_DIR`-unset fallback is the world-traversable
/// `/tmp/yerd-$UID`, so a directory that cannot be locked down is genuinely
/// unsafe to use. On Windows both `%LOCALAPPDATA%\yerd` and `%TEMP%\yerd` are
/// already per-user under their default ACL, so this is defence in depth and
/// must never be able to brick daemon startup.
#[cfg(windows)]
fn apply_dacl(path: &Path, recursive_inherit: bool) {
    let sid = match yerd_platform::current_user_sid() {
        Ok(sid) => sid,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "cannot resolve the current user SID; leaving the default ACL in place"
            );
            return;
        }
    };
    run_icacls(path, icacls_reset_args(path));
    run_icacls(path, icacls_args(path, &sid, recursive_inherit));
}

/// One best-effort `icacls` invocation against `path`; every outcome but
/// success is logged and swallowed (see [`apply_dacl`]).
#[cfg(windows)]
fn run_icacls(path: &Path, args: Vec<String>) {
    let icacls = yerd_platform::system32_exe("icacls.exe");
    match yerd_platform::hidden_command(&icacls).args(args).output() {
        Ok(out) if out.status.success() => {}
        Ok(out) => tracing::warn!(
            path = %path.display(),
            status = %out.status,
            detail = %String::from_utf8_lossy(&out.stdout).trim(),
            "icacls could not restrict the DACL; leaving the default ACL in place"
        ),
        Err(e) => tracing::warn!(
            path = %path.display(),
            icacls = %icacls.display(),
            error = %e,
            "icacls could not be run; leaving the default ACL in place"
        ),
    }
}

/// No-op off Windows: DACLs do not exist there and the Unix path is covered by
/// `set_mode`.
#[cfg(not(windows))]
fn apply_dacl(_path: &Path, _recursive_inherit: bool) {}

/// Table tests for the pure argv builder. Ungated on purpose: the module below
/// is `#[cfg(all(test, unix))]`, so these would otherwise never run in CI on the
/// one OS the argv is actually for.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod icacls_args_tests {
    use super::*;

    const SID: &str = "S-1-5-21-1111111111-2222222222-3333333333-1001";

    #[test]
    fn grant_carries_inheritance_flags_only_for_directories() {
        let cases: &[(&str, bool, &str)] = &[
            (
                "a directory's ACE must be inheritable by its children",
                true,
                "*S-1-5-21-1111111111-2222222222-3333333333-1001:(OI)(CI)F",
            ),
            (
                "a file has no children to inherit it",
                false,
                "*S-1-5-21-1111111111-2222222222-3333333333-1001:F",
            ),
        ];
        for (why, recursive_inherit, want_grant) in cases {
            let args = icacls_args(Path::new("runtime"), SID, *recursive_inherit);
            assert_eq!(
                args,
                vec![
                    "runtime".to_owned(),
                    "/inheritance:r".to_owned(),
                    "/grant:r".to_owned(),
                    (*want_grant).to_owned(),
                ],
                "{why}"
            );
        }
    }

    #[test]
    fn sid_is_star_prefixed_so_icacls_does_not_read_it_as_a_name() {
        for recursive_inherit in [true, false] {
            let args = icacls_args(Path::new("x"), SID, recursive_inherit);
            let grant = args.last().unwrap();
            assert!(
                grant.starts_with(&format!("*{SID}:")),
                "grant {grant:?} must star-prefix the SID"
            );
        }
    }

    #[test]
    fn path_is_one_unquoted_element() {
        let args = icacls_args(Path::new("a dir/with spaces"), SID, true);
        assert_eq!(
            args.first().map(String::as_str),
            Some("a dir/with spaces"),
            "the spawner quotes the argument; the builder must not"
        );
    }

    /// The reset pass clears the DACL and nothing else: pairing `/reset` with
    /// `/inheritance` or `/grant` makes icacls refuse the whole invocation.
    #[test]
    fn reset_is_its_own_invocation() {
        let args = icacls_reset_args(Path::new("runtime"));
        assert_eq!(args, vec!["runtime".to_owned(), "/reset".to_owned()]);
    }
}

/// The DACL actually landing on disk. Windows-only because it shells out to the
/// real `icacls`.
#[cfg(all(test, windows))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod windows_dacl_tests {
    use super::*;

    /// ACE lines in `icacls` output are the ones carrying a `:(...)` permission
    /// set, and an inherited ACE is flagged `(I)`. Nothing here asserts on the
    /// principal name: `icacls` prints it localized (`VORDEFINIERT\`,
    /// `AUTORITE NT\`), so matching that text would pass or fail by accident on
    /// a non-English host. Principal identity is a manual dev-host check.
    #[test]
    fn create_private_dir_leaves_exactly_one_non_inherited_ace() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("runtime");
        create_private_dir(&dir).unwrap();

        let out = std::process::Command::new(yerd_platform::system32_exe("icacls.exe"))
            .arg(&dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "icacls failed: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let aces: Vec<&str> = stdout.lines().filter(|l| l.contains(":(")).collect();
        assert_eq!(aces.len(), 1, "expected one ACE line, got {aces:?}");
        assert!(
            !aces[0].contains("(I)"),
            "the surviving ACE is still inherited: {aces:?}"
        );
    }
}

#[cfg(all(test, unix))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn create_private_dir_is_0700() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("runtime");
        create_private_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn create_private_dir_tightens_existing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("runtime");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        create_private_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn restrict_to_owner_is_0600() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("ca.key.pem");
        std::fs::write(&file, b"secret").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        restrict_to_owner(&file).unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn restrict_writes_to_owner_strips_group_world_write() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("ca.cert.pem");
        std::fs::write(&file, b"public cert").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o664)).unwrap();
        restrict_writes_to_owner(&file).unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o644,
            "cert must be world-readable but owner-write only"
        );
        assert_eq!(mode & 0o022, 0);
    }
}
