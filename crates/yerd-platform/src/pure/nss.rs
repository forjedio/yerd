//! Pure decision helpers for per-user NSS trust (`certutil`).
//!
//! On Linux, Chromium-family browsers (Brave, Chrome, Chromium, Edge) and
//! Firefox do not consult the system CA store; each reads a per-user **NSS
//! database**. Chromium-family share `~/.pki/nssdb`; Firefox keeps one
//! `cert9.db` per profile. Making the Yerd CA trusted there requires
//! `certutil` from `libnss3-tools`.
//!
//! This module derives *which* database directories to touch and builds the
//! `certutil` argv - both pure, so they are table-tested on every host. The
//! actual `read_dir`, existence checks, and process spawning stay in the OS
//! impl (`os::linux` / `os::macos`), mirroring the pure/edge split used by
//! [`crate::pure::system_roots`].
//!
//! v1 is **sql-only**: modern Firefox and Chromium use the `sql:` (`cert9.db`)
//! store. Legacy `cert8.db` (dbm) databases are not written.

use std::path::{Path, PathBuf};

use crate::pure::firefox;

/// Nickname the Yerd CA is stored under in every NSS database. Stable so that
/// re-trusting (or a CA rotation) overwrites the same entry rather than
/// accumulating duplicates: the edge deletes this nickname before adding.
pub const NICKNAME: &str = "Yerd Local CA";

/// NSS trust flags for a locally-trusted TLS server-auth root. `C` in the SSL
/// trust field marks the certificate a trusted CA (the same flag `mkcert`
/// uses). Peer flags (`p`/`P`) must not be used - they would not let a served
/// leaf chain to this root.
pub const TRUST_FLAGS: &str = "C,,";

/// An NSS database directory. Only the `sql:` form is produced in v1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NssDb {
    /// A modern `sql:`-prefixed database (`cert9.db`).
    Sql(PathBuf),
}

impl NssDb {
    /// The directory this database lives in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        match self {
            Self::Sql(p) => p,
        }
    }

    /// The `-d` argument value `certutil` expects (`sql:<dir>`).
    #[must_use]
    pub fn db_arg(&self) -> String {
        match self {
            Self::Sql(p) => format!("sql:{}", p.display()),
        }
    }
}

/// `certutil` argv to add the CA (from a PEM file at `ca_path`) as a trusted
/// root. Delete first (see [`delete_args`]) for idempotency.
#[must_use]
pub fn add_args(db: &NssDb, ca_path: &Path) -> Vec<String> {
    vec![
        "-d".to_owned(),
        db.db_arg(),
        "-A".to_owned(),
        "-n".to_owned(),
        NICKNAME.to_owned(),
        "-t".to_owned(),
        TRUST_FLAGS.to_owned(),
        "-i".to_owned(),
        ca_path.display().to_string(),
    ]
}

/// `certutil` argv to delete any certificate stored under [`NICKNAME`].
#[must_use]
pub fn delete_args(db: &NssDb) -> Vec<String> {
    vec![
        "-d".to_owned(),
        db.db_arg(),
        "-D".to_owned(),
        "-n".to_owned(),
        NICKNAME.to_owned(),
    ]
}

/// `certutil` argv to print (ASCII/PEM) the certificate stored under
/// [`NICKNAME`], for fingerprint identity comparison. Exits non-zero when no
/// such entry exists.
#[must_use]
pub fn list_pem_args(db: &NssDb) -> Vec<String> {
    vec![
        "-d".to_owned(),
        db.db_arg(),
        "-L".to_owned(),
        "-n".to_owned(),
        NICKNAME.to_owned(),
        "-a".to_owned(),
    ]
}

/// `certutil` argv to initialise a fresh, password-less database in `dir`.
/// Used only when `~/.pki/nssdb` does not yet exist.
#[must_use]
pub fn init_args(dir: &Path) -> Vec<String> {
    vec![
        "-d".to_owned(),
        format!("sql:{}", dir.display()),
        "-N".to_owned(),
        "--empty-password".to_owned(),
    ]
}

/// The shared Chromium-family NSS database directory (`<home>/.pki/nssdb`).
#[must_use]
pub fn pki_nssdb_dir(home: &Path) -> PathBuf {
    home.join(".pki/nssdb")
}

/// Every `.pki/nssdb` directory to consider, given the home dir and the app
/// directory names discovered under `~/snap` and `~/.var/app`.
///
/// Native Chromium-family use `<home>/.pki/nssdb`. Snap apps persist data in
/// either `common/` (survives refresh) or `current/` (migrated on refresh, and
/// where Chromium-snap actually reads), so both are probed. Flatpak apps use
/// `<home>/.var/app/<id>/.pki/nssdb`.
#[must_use]
pub fn pki_nssdb_candidates(
    home: &Path,
    snap_apps: &[String],
    flatpak_apps: &[String],
) -> Vec<PathBuf> {
    let mut out = vec![pki_nssdb_dir(home)];
    let snap_root = home.join("snap");
    for app in snap_apps {
        out.push(snap_root.join(app).join("common/.pki/nssdb"));
        out.push(snap_root.join(app).join("current/.pki/nssdb"));
    }
    let flatpak_root = home.join(".var/app");
    for app in flatpak_apps {
        out.push(flatpak_root.join(app).join(".pki/nssdb"));
    }
    out
}

/// Every Firefox profiles-root directory (the parent of the per-profile dirs,
/// i.e. where `profiles.ini` lives) to consider. The edge reads each
/// `profiles.ini` and calls [`firefox_profile_dirs`].
#[must_use]
pub fn firefox_root_candidates(
    home: &Path,
    snap_apps: &[String],
    flatpak_apps: &[String],
) -> Vec<PathBuf> {
    let mut out = vec![home.join(".mozilla/firefox")];
    let snap_root = home.join("snap");
    for app in snap_apps {
        out.push(snap_root.join(app).join("common/.mozilla/firefox"));
        out.push(snap_root.join(app).join("current/.mozilla/firefox"));
    }
    let flatpak_root = home.join(".var/app");
    for app in flatpak_apps {
        out.push(flatpak_root.join(app).join(".mozilla/firefox"));
    }
    out
}

/// Resolve the per-profile directories under a Firefox `profiles_root` from the
/// text of its `profiles.ini`. Relative `Path=` entries are joined against
/// `profiles_root`; absolute entries are used as-is. The edge then keeps only
/// those that actually contain a `cert9.db`.
#[must_use]
pub fn firefox_profile_dirs(profiles_root: &Path, profiles_ini_text: &str) -> Vec<PathBuf> {
    firefox::parse_profiles_ini(profiles_ini_text)
        .into_iter()
        .map(|p| {
            if p.is_relative {
                profiles_root.join(p.path)
            } else {
                PathBuf::from(p.path)
            }
        })
        .collect()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/alice")
    }

    #[test]
    fn add_args_uses_c_flag_and_sql_prefix() {
        let db = NssDb::Sql(PathBuf::from("/home/alice/.pki/nssdb"));
        let args = add_args(&db, Path::new("/data/ca.cert.pem"));
        assert_eq!(
            args,
            vec![
                "-d",
                "sql:/home/alice/.pki/nssdb",
                "-A",
                "-n",
                "Yerd Local CA",
                "-t",
                "C,,",
                "-i",
                "/data/ca.cert.pem",
            ]
        );
    }

    #[test]
    fn delete_and_list_and_init_args() {
        let db = NssDb::Sql(PathBuf::from("/db"));
        assert_eq!(
            delete_args(&db),
            vec!["-d", "sql:/db", "-D", "-n", "Yerd Local CA"]
        );
        assert_eq!(
            list_pem_args(&db),
            vec!["-d", "sql:/db", "-L", "-n", "Yerd Local CA", "-a"]
        );
        assert_eq!(
            init_args(Path::new("/db")),
            vec!["-d", "sql:/db", "-N", "--empty-password"]
        );
    }

    #[test]
    fn pki_candidates_cover_native_snap_both_roots_and_flatpak() {
        let got = pki_nssdb_candidates(
            &home(),
            &["firefox".to_owned(), "chromium".to_owned()],
            &["com.brave.Browser".to_owned()],
        );
        assert_eq!(got[0], PathBuf::from("/home/alice/.pki/nssdb"));
        assert!(got.contains(&PathBuf::from(
            "/home/alice/snap/chromium/common/.pki/nssdb"
        )));
        assert!(got.contains(&PathBuf::from(
            "/home/alice/snap/chromium/current/.pki/nssdb"
        )));
        assert!(got.contains(&PathBuf::from(
            "/home/alice/.var/app/com.brave.Browser/.pki/nssdb"
        )));
    }

    #[test]
    fn firefox_roots_cover_native_snap_common_and_flatpak() {
        let got = firefox_root_candidates(
            &home(),
            &["firefox".to_owned()],
            &["org.mozilla.firefox".to_owned()],
        );
        assert_eq!(got[0], PathBuf::from("/home/alice/.mozilla/firefox"));
        assert!(got.contains(&PathBuf::from(
            "/home/alice/snap/firefox/common/.mozilla/firefox"
        )));
        assert!(got.contains(&PathBuf::from(
            "/home/alice/.var/app/org.mozilla.firefox/.mozilla/firefox"
        )));
    }

    #[test]
    fn firefox_profile_dirs_joins_relative_against_root() {
        let ini = "[Profile0]\nIsRelative=1\nPath=abc.default\n";
        let dirs = firefox_profile_dirs(Path::new("/home/alice/.mozilla/firefox"), ini);
        assert_eq!(
            dirs,
            vec![PathBuf::from("/home/alice/.mozilla/firefox/abc.default")]
        );
    }

    #[test]
    fn firefox_profile_dirs_keeps_absolute_paths() {
        let ini = "[Profile0]\nIsRelative=0\nPath=/custom/profile\n";
        let dirs = firefox_profile_dirs(Path::new("/home/alice/.mozilla/firefox"), ini);
        assert_eq!(dirs, vec![PathBuf::from("/custom/profile")]);
    }

    #[test]
    fn no_snap_or_flatpak_apps_yields_native_only() {
        assert_eq!(pki_nssdb_candidates(&home(), &[], &[]).len(), 1);
        assert_eq!(firefox_root_candidates(&home(), &[], &[]).len(), 1);
    }
}
