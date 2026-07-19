//! Per-user NSS trust orchestration, shared by the Linux and macOS impls.
//!
//! The side effects - running `certutil` and probing the filesystem - are
//! injected behind [`CertutilRunner`] and [`NssFs`], so the discover -> argv ->
//! run -> aggregate logic is unit-tested in-memory with fakes (the definition
//! of done requires every side-effecting path be behind a trait and tested with
//! a fake). The real impls ([`RealCertutil`], [`RealNssFs`]) live under
//! `#[cfg(unix)]` and are the thin edge.
//!
//! Path derivation and the `certutil` argv are pure ([`crate::pure::nss`]).

use std::path::{Path, PathBuf};

use crate::pure::nss::{self, NssDb};
use crate::trust_store::{BrowserCaTrust, CaFingerprint, NssFailure, NssOutcome};

/// Result of one `certutil` invocation.
pub struct RunResult {
    /// Process exit code (`-1` if terminated by signal).
    pub code: i32,
    /// Captured stdout (used for the `-L -a` PEM readback).
    pub stdout: Vec<u8>,
}

impl RunResult {
    fn ok(&self) -> bool {
        self.code == 0
    }
}

/// Runs `certutil`. Injected so orchestration is testable without the binary.
pub trait CertutilRunner {
    /// Whether `certutil` is installed and runnable.
    fn available(&self) -> bool;
    /// Run `certutil` with `args`.
    fn run(&self, args: &[String]) -> RunResult;
}

/// Filesystem probes the orchestration needs. Injected for testing.
pub trait NssFs {
    /// The invoking user's home directory (from `$HOME`; never `SUDO_*`).
    fn home(&self) -> Option<PathBuf>;
    /// Whether `path` is an existing directory.
    fn dir_exists(&self, path: &Path) -> bool;
    /// Whether `path` is an existing file.
    fn file_exists(&self, path: &Path) -> bool;
    /// Immediate sub-directory names of `dir` (empty if `dir` is absent).
    fn list_subdirs(&self, dir: &Path) -> Vec<String>;
    /// Read a file to a `String`, or `None` if absent/unreadable.
    fn read_to_string(&self, path: &Path) -> Option<String>;
    /// Create `dir` and parents (mode 0700 on the real impl).
    fn create_dir_all(&self, dir: &Path) -> std::io::Result<()>;
}

fn empty_outcome() -> NssOutcome {
    NssOutcome {
        profiles_attempted: 0,
        profiles_succeeded: 0,
        failures: vec![],
        certutil_missing: false,
    }
}

/// The platforms whose per-user NSS layout differs. On Linux, Chromium-family
/// browsers use `~/.pki/nssdb`; on macOS they use the system keychain, so only
/// Firefox has an NSS store there - under a different profile root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Linux/other Unix: `~/.pki/nssdb` + `~/.mozilla/firefox` + Snap/Flatpak.
    Linux,
    /// macOS: Firefox only, under `~/Library/Application Support/Firefox`.
    Macos,
}

/// Discover every existing sql NSS database for `os`. Read-only; creates nothing.
fn discover(fs: &impl NssFs, home: &Path, os: Os) -> Vec<NssDb> {
    let mut dbs: Vec<NssDb> = Vec::new();
    let push = |db: NssDb, dbs: &mut Vec<NssDb>| {
        if !dbs.contains(&db) {
            dbs.push(db);
        }
    };

    let firefox_roots = match os {
        Os::Linux => {
            let snap_apps = fs.list_subdirs(&home.join("snap"));
            let flatpak_apps = fs.list_subdirs(&home.join(".var/app"));
            for dir in nss::pki_nssdb_candidates(home, &snap_apps, &flatpak_apps) {
                if fs.dir_exists(&dir) {
                    push(NssDb::Sql(dir), &mut dbs);
                }
            }
            nss::firefox_root_candidates(home, &snap_apps, &flatpak_apps)
        }
        Os::Macos => vec![nss::macos_firefox_root(home)],
    };
    for root in firefox_roots {
        let Some(ini) = fs.read_to_string(&root.join("profiles.ini")) else {
            continue;
        };
        for profile in nss::firefox_profile_dirs(&root, &ini) {
            if fs.file_exists(&profile.join("cert9.db")) {
                push(NssDb::Sql(profile), &mut dbs);
            }
        }
    }
    dbs
}

/// Install the CA (PEM at `ca_path`) into every browser NSS database. On Linux,
/// the shared Chromium store `~/.pki/nssdb` is created and initialised first if
/// absent (so a browser installed-but-never-launched still trusts the CA on
/// first run); macOS has no such shared store.
pub fn install(
    fs: &impl NssFs,
    runner: &impl CertutilRunner,
    ca_path: &Path,
    os: Os,
) -> NssOutcome {
    if !runner.available() {
        return NssOutcome {
            certutil_missing: true,
            ..empty_outcome()
        };
    }
    let Some(home) = fs.home() else {
        return empty_outcome();
    };

    if os == Os::Linux {
        let pki = nss::pki_nssdb_dir(&home);
        if !fs.dir_exists(&pki) && fs.create_dir_all(&pki).is_ok() {
            let _ = runner.run(&nss::init_args(&pki));
        }
    }

    let mut out = empty_outcome();
    for db in discover(fs, &home, os) {
        let _ = runner.run(&nss::delete_args(&db));
        let res = runner.run(&nss::add_args(&db, ca_path));
        out.profiles_attempted += 1;
        if res.ok() {
            out.profiles_succeeded += 1;
        } else {
            out.failures
                .push((db.dir().to_path_buf(), NssFailure::CertutilExit(res.code)));
        }
    }
    out
}

/// Remove the Yerd CA from every discovered browser NSS database.
pub fn uninstall(fs: &impl NssFs, runner: &impl CertutilRunner, os: Os) -> NssOutcome {
    if !runner.available() {
        return NssOutcome {
            certutil_missing: true,
            ..empty_outcome()
        };
    }
    let Some(home) = fs.home() else {
        return empty_outcome();
    };

    let mut out = empty_outcome();
    for db in discover(fs, &home, os) {
        let res = runner.run(&nss::delete_args(&db));
        out.profiles_attempted += 1;
        // Deleting a missing entry exits non-zero (nothing to remove) - still a
        // success for us. Only a spawn failure (code -1) counts as a failure.
        if res.code == -1 {
            out.failures
                .push((db.dir().to_path_buf(), NssFailure::CertutilExit(res.code)));
        } else {
            out.profiles_succeeded += 1;
        }
    }
    out
}

/// Whether a single NSS database `db` trusts the CA with fingerprint `fp`:
/// reads back the cert stored under our nickname and compares fingerprints.
fn db_trusts(runner: &impl CertutilRunner, db: &NssDb, fp: &CaFingerprint) -> bool {
    let res = runner.run(&nss::list_pem_args(db));
    if !res.ok() {
        return false;
    }
    let Ok(pem) = String::from_utf8(res.stdout) else {
        return false;
    };
    CaFingerprint::from_pem(&pem).as_ref() == Some(fp)
}

/// Whether the browser NSS stores trust the CA with fingerprint `fp`.
///
/// Requires **every** discovered store to trust it: a single untrusted store
/// (e.g. a Firefox profile created after the last trust run) yields `Untrusted`
/// so the doctor warning fires rather than being masked by another store that
/// happens to trust the CA.
pub fn browser_trust(
    fs: &impl NssFs,
    runner: &impl CertutilRunner,
    fp: &CaFingerprint,
    os: Os,
) -> BrowserCaTrust {
    let Some(home) = fs.home() else {
        return BrowserCaTrust::Trusted;
    };
    let dbs = discover(fs, &home, os);
    if dbs.is_empty() {
        // No browser NSS store exists - nothing to trust, so don't nag.
        return BrowserCaTrust::Trusted;
    }
    if !runner.available() {
        return BrowserCaTrust::ToolMissing;
    }
    if dbs.iter().all(|db| db_trusts(runner, db, fp)) {
        BrowserCaTrust::Trusted
    } else {
        BrowserCaTrust::Untrusted
    }
}

// ---- real (edge) impls ----------------------------------------------------

#[cfg(unix)]
pub use real::{real_browser_trust, real_install, real_uninstall};

#[cfg(unix)]
mod real {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{CertutilRunner, NssFs, Os, RunResult};
    use crate::trust_store::{BrowserCaTrust, CaFingerprint, NssOutcome};

    /// The layout of the host we are running on.
    const fn current_os() -> Os {
        #[cfg(target_os = "macos")]
        {
            Os::Macos
        }
        #[cfg(not(target_os = "macos"))]
        {
            Os::Linux
        }
    }

    /// `certutil` located at an absolute path (resolved once). Absolute-path
    /// resolution mirrors the `/usr/bin/id` / `/bin/ps` precedent - a stripped
    /// `PATH` under a service manager must not hide the tool.
    struct RealCertutil {
        path: Option<PathBuf>,
    }

    impl RealCertutil {
        fn resolve() -> Self {
            let common = Path::new("/usr/bin/certutil");
            if common.is_file() {
                return Self {
                    path: Some(common.to_path_buf()),
                };
            }
            let path = std::env::var_os("PATH").and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|dir| dir.join("certutil"))
                    .find(|candidate| candidate.is_file())
            });
            Self { path }
        }
    }

    impl CertutilRunner for RealCertutil {
        fn available(&self) -> bool {
            self.path.is_some()
        }

        fn run(&self, args: &[String]) -> RunResult {
            let Some(bin) = self.path.as_ref() else {
                return RunResult {
                    code: -1,
                    stdout: Vec::new(),
                };
            };
            match Command::new(bin).args(args).output() {
                Ok(out) => RunResult {
                    code: out.status.code().unwrap_or(-1),
                    stdout: out.stdout,
                },
                Err(_) => RunResult {
                    code: -1,
                    stdout: Vec::new(),
                },
            }
        }
    }

    struct RealNssFs;

    impl NssFs for RealNssFs {
        fn home(&self) -> Option<PathBuf> {
            std::env::var_os("HOME").map(PathBuf::from)
        }

        fn dir_exists(&self, path: &Path) -> bool {
            path.is_dir()
        }

        fn file_exists(&self, path: &Path) -> bool {
            path.is_file()
        }

        fn list_subdirs(&self, dir: &Path) -> Vec<String> {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return Vec::new();
            };
            entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        }

        fn read_to_string(&self, path: &Path) -> Option<String> {
            std::fs::read_to_string(path).ok()
        }

        fn create_dir_all(&self, dir: &Path) -> std::io::Result<()> {
            std::fs::create_dir_all(dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
            Ok(())
        }
    }

    /// Install the CA at `ca_path` into every per-user browser NSS store.
    #[must_use]
    pub fn real_install(ca_path: &Path) -> NssOutcome {
        super::install(&RealNssFs, &RealCertutil::resolve(), ca_path, current_os())
    }

    /// Remove the Yerd CA from every per-user browser NSS store.
    #[must_use]
    pub fn real_uninstall() -> NssOutcome {
        super::uninstall(&RealNssFs, &RealCertutil::resolve(), current_os())
    }

    /// Probe whether browsers trust the CA with fingerprint `fp`.
    #[must_use]
    pub fn real_browser_trust(fp: &CaFingerprint) -> BrowserCaTrust {
        super::browser_trust(&RealNssFs, &RealCertutil::resolve(), fp, current_os())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    use super::*;

    #[derive(Default)]
    struct FakeFs {
        home: Option<PathBuf>,
        dirs: HashSet<PathBuf>,
        files: HashMap<PathBuf, String>,
        created: RefCell<Vec<PathBuf>>,
    }

    impl NssFs for FakeFs {
        fn home(&self) -> Option<PathBuf> {
            self.home.clone()
        }
        fn dir_exists(&self, path: &Path) -> bool {
            self.dirs.contains(path)
        }
        fn file_exists(&self, path: &Path) -> bool {
            self.files.contains_key(path)
        }
        fn list_subdirs(&self, dir: &Path) -> Vec<String> {
            self.dirs
                .iter()
                .filter_map(|d| d.parent().filter(|p| *p == dir).and(d.file_name()))
                .filter_map(|n| n.to_str().map(str::to_owned))
                .collect()
        }
        fn read_to_string(&self, path: &Path) -> Option<String> {
            self.files.get(path).cloned()
        }
        fn create_dir_all(&self, dir: &Path) -> std::io::Result<()> {
            self.created.borrow_mut().push(dir.to_path_buf());
            Ok(())
        }
    }

    struct FakeRunner {
        available: bool,
        calls: RefCell<Vec<Vec<String>>>,
        list_pem: Option<String>,
        add_fails: bool,
    }

    impl FakeRunner {
        fn new(available: bool) -> Self {
            Self {
                available,
                calls: RefCell::new(vec![]),
                list_pem: None,
                add_fails: false,
            }
        }
    }

    impl CertutilRunner for FakeRunner {
        fn available(&self) -> bool {
            self.available
        }
        fn run(&self, args: &[String]) -> RunResult {
            self.calls.borrow_mut().push(args.to_vec());
            if args.iter().any(|a| a == "-L") {
                return match &self.list_pem {
                    Some(pem) => RunResult {
                        code: 0,
                        stdout: pem.clone().into_bytes(),
                    },
                    None => RunResult {
                        code: 255,
                        stdout: vec![],
                    },
                };
            }
            if args.iter().any(|a| a == "-A") && self.add_fails {
                return RunResult {
                    code: 255,
                    stdout: vec![],
                };
            }
            RunResult {
                code: 0,
                stdout: vec![],
            }
        }
    }

    fn home() -> PathBuf {
        PathBuf::from("/home/alice")
    }

    fn fs_with_pki() -> FakeFs {
        let mut fs = FakeFs {
            home: Some(home()),
            ..FakeFs::default()
        };
        fs.dirs.insert(home().join(".pki/nssdb"));
        fs
    }

    #[test]
    fn install_missing_certutil_flags_it() {
        let out = install(
            &fs_with_pki(),
            &FakeRunner::new(false),
            Path::new("/ca.pem"),
            Os::Linux,
        );
        assert!(out.certutil_missing);
        assert_eq!(out.profiles_attempted, 0);
    }

    #[test]
    fn install_adds_to_existing_pki_with_delete_then_add() {
        let fs = fs_with_pki();
        let runner = FakeRunner::new(true);
        let out = install(&fs, &runner, Path::new("/ca.pem"), Os::Linux);
        assert_eq!(out.profiles_attempted, 1);
        assert_eq!(out.profiles_succeeded, 1);
        let calls = runner.calls.borrow();
        // delete precedes add for idempotency.
        let del = calls.iter().position(|c| c.contains(&"-D".to_owned()));
        let add = calls.iter().position(|c| c.contains(&"-A".to_owned()));
        assert!(del < add);
    }

    #[test]
    fn install_creates_and_inits_absent_pki() {
        let fs = FakeFs {
            home: Some(home()),
            ..FakeFs::default()
        };
        let runner = FakeRunner::new(true);
        let _ = install(&fs, &runner, Path::new("/ca.pem"), Os::Linux);
        assert_eq!(fs.created.borrow().as_slice(), &[home().join(".pki/nssdb")]);
        assert!(runner
            .calls
            .borrow()
            .iter()
            .any(|c| c.contains(&"-N".to_owned())));
    }

    #[test]
    fn install_records_failure_exit() {
        let fs = fs_with_pki();
        let mut runner = FakeRunner::new(true);
        runner.add_fails = true;
        let out = install(&fs, &runner, Path::new("/ca.pem"), Os::Linux);
        assert_eq!(out.profiles_succeeded, 0);
        assert_eq!(out.failures.len(), 1);
    }

    #[test]
    fn discover_finds_firefox_profile_with_cert9() {
        let mut fs = FakeFs {
            home: Some(home()),
            ..FakeFs::default()
        };
        let ff_root = home().join(".mozilla/firefox");
        fs.files.insert(
            ff_root.join("profiles.ini"),
            "[Profile0]\nIsRelative=1\nPath=abc.default\n".to_owned(),
        );
        fs.files
            .insert(ff_root.join("abc.default/cert9.db"), String::new());
        let runner = FakeRunner::new(true);
        let out = install(&fs, &runner, Path::new("/ca.pem"), Os::Linux);
        assert_eq!(out.profiles_attempted, 1);
    }

    #[test]
    fn browser_trust_none_when_no_dbs() {
        let fs = FakeFs {
            home: Some(home()),
            ..FakeFs::default()
        };
        let fp = CaFingerprint::new([7u8; 32]);
        assert_eq!(
            browser_trust(&fs, &FakeRunner::new(true), &fp, Os::Linux),
            BrowserCaTrust::Trusted
        );
    }

    #[test]
    fn browser_trust_tool_missing_when_db_exists_but_no_certutil() {
        let fp = CaFingerprint::new([7u8; 32]);
        assert_eq!(
            browser_trust(&fs_with_pki(), &FakeRunner::new(false), &fp, Os::Linux),
            BrowserCaTrust::ToolMissing
        );
    }

    #[test]
    fn browser_trust_untrusted_when_absent() {
        let fp = CaFingerprint::new([7u8; 32]);
        assert_eq!(
            browser_trust(&fs_with_pki(), &FakeRunner::new(true), &fp, Os::Linux),
            BrowserCaTrust::Untrusted
        );
    }

    #[test]
    fn browser_trust_trusted_on_fingerprint_match() {
        // Build a real self-signed CA so its PEM fingerprint round-trips.
        let pem = sample_ca_pem();
        let fp = CaFingerprint::from_pem(&pem).unwrap();
        let mut runner = FakeRunner::new(true);
        runner.list_pem = Some(pem);
        assert_eq!(
            browser_trust(&fs_with_pki(), &runner, &fp, Os::Linux),
            BrowserCaTrust::Trusted
        );
    }

    #[test]
    fn macos_uses_library_firefox_and_no_pki() {
        // macOS discovery must find the Library Firefox profile and must NOT
        // create or touch ~/.pki/nssdb (no macOS browser reads it).
        let mut fs = FakeFs {
            home: Some(home()),
            ..FakeFs::default()
        };
        let ff_root = home().join("Library/Application Support/Firefox");
        fs.files.insert(
            ff_root.join("profiles.ini"),
            "[Profile0]\nIsRelative=1\nPath=abc.default\n".to_owned(),
        );
        fs.files
            .insert(ff_root.join("abc.default/cert9.db"), String::new());
        let runner = FakeRunner::new(true);
        let out = install(&fs, &runner, Path::new("/ca.pem"), Os::Macos);
        assert_eq!(out.profiles_attempted, 1);
        assert!(
            fs.created.borrow().is_empty(),
            "must not create ~/.pki/nssdb on macOS"
        );
        assert!(!runner
            .calls
            .borrow()
            .iter()
            .any(|c| c.iter().any(|a| a.contains(".pki/nssdb"))));
    }

    #[test]
    fn browser_trust_untrusted_when_any_store_untrusted() {
        // Two stores exist; only ~/.pki/nssdb trusts the CA (list_pem set), the
        // Firefox profile does not (its -L is answered the same way by the fake,
        // so instead use a runner that only trusts when the db is the pki one).
        let pem = sample_ca_pem();
        let fp = CaFingerprint::from_pem(&pem).unwrap();
        let mut fs = fs_with_pki();
        let ff_root = home().join(".mozilla/firefox");
        fs.files.insert(
            ff_root.join("profiles.ini"),
            "[Profile0]\nIsRelative=1\nPath=abc.default\n".to_owned(),
        );
        fs.files
            .insert(ff_root.join("abc.default/cert9.db"), String::new());
        // Fake trusts only the pki store: -L returns the PEM only for that dir.
        let runner = SelectiveRunner {
            trusted_dir: home().join(".pki/nssdb").display().to_string(),
            pem,
        };
        assert_eq!(
            browser_trust(&fs, &runner, &fp, Os::Linux),
            BrowserCaTrust::Untrusted
        );
    }

    /// A runner whose `-L` returns the CA PEM only for the one db dir it trusts.
    struct SelectiveRunner {
        trusted_dir: String,
        pem: String,
    }

    impl CertutilRunner for SelectiveRunner {
        fn available(&self) -> bool {
            true
        }
        fn run(&self, args: &[String]) -> RunResult {
            if args.iter().any(|a| a == "-L") {
                let for_trusted = args
                    .iter()
                    .any(|a| a == &format!("sql:{}", self.trusted_dir));
                if for_trusted {
                    return RunResult {
                        code: 0,
                        stdout: self.pem.clone().into_bytes(),
                    };
                }
                return RunResult {
                    code: 255,
                    stdout: vec![],
                };
            }
            RunResult {
                code: 0,
                stdout: vec![],
            }
        }
    }

    fn sample_ca_pem() -> String {
        // A throwaway PEM; fingerprint identity is over its DER, so any real
        // cert works. Reuse yerd-tls to mint one.
        let now = time::OffsetDateTime::now_utc();
        let v =
            yerd_tls::Validity::new(now - time::Duration::days(1), now + time::Duration::days(1))
                .unwrap();
        yerd_tls::CertAuthority::generate("Sample CA", v)
            .unwrap()
            .cert_pem()
            .to_owned()
    }
}
