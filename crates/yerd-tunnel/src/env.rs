//! The child environment `cloudflared` is given, decided per host.
//!
//! The supervised child runs with a cleared environment so a stray
//! `TUNNEL_*` or credential variable in the daemon's own environment can never
//! reach it. That means everything it needs has to be listed here, and the list
//! differs by host: the Unix baseline is not the Windows one.
//!
//! Pure, sync and I/O-free, so both arms are table-tested on every OS rather
//! than only where they run.

use std::ffi::OsString;
use std::path::Path;

/// Variables pinned to Yerd's own tunnel directory, so a separately installed
/// `cloudflared` and the real user's home are never consulted.
///
/// `HOME` is honoured on Windows as well as Unix: `cloudflared` expands `~`
/// through `mitchellh/go-homedir`, whose Windows lookup prefers `%HOME%`, and
/// its default config directory is `~/.cloudflared` on every OS. Windows
/// additionally gets `CFDPATH`, which overrides `cloudflared`'s Windows-only
/// default config directory.
#[must_use]
pub fn pinned_vars(tunnel_dir: &Path, windows: bool) -> Vec<(OsString, OsString)> {
    let mut vars = vec![(OsString::from("HOME"), OsString::from(tunnel_dir))];
    if windows {
        vars.push((
            OsString::from("CFDPATH"),
            OsString::from(tunnel_dir.join(".cloudflared")),
        ));
    }
    vars
}

/// Variables forwarded from the daemon's environment into the cleared child.
///
/// The Unix list is the long-standing one. The Windows list drops `TMPDIR` and
/// the `SSL_CERT_*` pair, which are inert there, and adds the baseline a Windows
/// process is built to assume: `SystemRoot`, `SystemDrive` and `windir`, plus
/// `TEMP` and `TMP` without which `GetTempPath` falls back to the unwritable
/// Windows directory.
///
/// `USERPROFILE`, `HOMEDRIVE`/`HOMEPATH` and `ProgramFiles(x86)` are
/// deliberately excluded, so a missing `HOME` fails loudly rather than quietly
/// resolving to the real user's home or to an MSI-installed `cloudflared`'s
/// configuration.
#[must_use]
pub fn forwarded_keys(windows: bool) -> &'static [&'static str] {
    if windows {
        &["PATH", "SystemRoot", "SystemDrive", "windir", "TEMP", "TMP"]
    } else {
        &["PATH", "TMPDIR", "SSL_CERT_FILE", "SSL_CERT_DIR"]
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
    use super::*;

    #[test]
    fn unix_pins_only_home() {
        let vars = pinned_vars(Path::new("/data/tunnel"), false);
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].0, OsString::from("HOME"));
    }

    #[test]
    fn windows_also_pins_the_config_dir() {
        let dir = Path::new("C:/data/tunnel");
        let vars = pinned_vars(dir, true);
        let keys: Vec<&OsString> = vars.iter().map(|(k, _)| k).collect();
        assert!(keys.contains(&&OsString::from("HOME")));
        assert!(keys.contains(&&OsString::from("CFDPATH")));
        let cfd = vars
            .iter()
            .find(|(k, _)| k == &OsString::from("CFDPATH"))
            .map(|(_, v)| v.clone())
            .unwrap();
        assert_eq!(cfd, OsString::from(dir.join(".cloudflared")));
    }

    /// The Unix list is a behavioural contract with the shipping daemon and must
    /// not drift.
    #[test]
    fn the_unix_forward_list_is_unchanged() {
        assert_eq!(
            forwarded_keys(false),
            ["PATH", "TMPDIR", "SSL_CERT_FILE", "SSL_CERT_DIR"]
        );
    }

    #[test]
    fn the_windows_forward_list_carries_the_system_baseline() {
        let keys = forwarded_keys(true);
        for required in ["PATH", "SystemRoot", "TEMP", "TMP"] {
            assert!(keys.contains(&required), "{required} must be forwarded");
        }
    }

    /// Forwarding either of these would let a missing `HOME` resolve silently to
    /// the real user's home or to a separately installed `cloudflared`.
    #[test]
    fn the_windows_forward_list_excludes_the_home_fallbacks() {
        let keys = forwarded_keys(true);
        for excluded in ["USERPROFILE", "HOMEDRIVE", "HOMEPATH", "ProgramFiles(x86)"] {
            assert!(
                !keys.contains(&excluded),
                "{excluded} must not be forwarded"
            );
        }
    }
}
