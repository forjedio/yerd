//! Root credentials for engines that need an access/secret key pair (the S3
//! gateway): generated once per install, persisted, reused on every start.
//!
//! An S3 gateway cannot run passwordless: `SigV4` signing is mandatory and
//! versitygw refuses to start without a root account. Yerd generates the pair on
//! first use and keeps it for the life of the install, so clients configured
//! against it keep working across restarts and version changes.
//!
//! The pair lives in its own owner-only file under the service's state dir
//! ([`crate::version::credentials_path`]), never in `config.toml`, which is
//! rewritten on unrelated changes and often pasted into bug reports. A file that
//! exists but cannot be parsed is an error naming the path, never a silent
//! regeneration: rotating keys behind the user's back would break every client.
//!
//! The pair reaches the gateway as environment, never argv, so it stays out of
//! `ps` output.

use std::fmt;
use std::io::{self, Write};
use std::path::Path;

use rand::distributions::Alphanumeric;
use rand::{Rng, RngCore};
use yerd_platform::PlatformDirs;

use crate::error::ServiceError;
use crate::version::credentials_path;

/// Prefix of every generated access key, so it is recognisable as Yerd's.
const ACCESS_KEY_PREFIX: &str = "yerd_";

/// Random characters after [`ACCESS_KEY_PREFIX`] in a generated access key.
const ACCESS_KEY_RANDOM_LEN: usize = 10;

/// Length of a generated secret key (AWS-style 40 characters).
const SECRET_KEY_LEN: usize = 40;

const ACCESS_KEY_FIELD: &str = "access_key";
const SECRET_KEY_FIELD: &str = "secret_key";

/// A root access/secret key pair. `Debug` redacts the secret.
#[derive(Clone, PartialEq, Eq)]
pub struct RootCredentials {
    /// The access key id (not secret).
    pub access_key: String,
    /// The secret access key.
    pub secret_key: String,
}

impl fmt::Debug for RootCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RootCredentials")
            .field("access_key", &self.access_key)
            .field("secret_key", &"<redacted>")
            .finish()
    }
}

fn random_alphanumeric(rng: &mut impl RngCore, len: usize) -> String {
    (0..len)
        .map(|_| char::from(rng.sample(Alphanumeric)))
        .collect()
}

/// Generate a pair from `rng`: access key `yerd_` plus 10 characters, secret of
/// 40 characters, both drawn from `[A-Za-z0-9]`.
#[must_use]
pub fn generate_with(rng: &mut impl RngCore) -> RootCredentials {
    let access_key = format!(
        "{ACCESS_KEY_PREFIX}{}",
        random_alphanumeric(rng, ACCESS_KEY_RANDOM_LEN)
    );
    let secret_key = random_alphanumeric(rng, SECRET_KEY_LEN);
    RootCredentials {
        access_key,
        secret_key,
    }
}

/// Generate a pair from the OS random source.
#[must_use]
pub fn generate() -> RootCredentials {
    generate_with(&mut rand::rngs::OsRng)
}

/// Render the two-line `access_key=…` / `secret_key=…` file body.
#[must_use]
pub fn render(creds: &RootCredentials) -> String {
    format!(
        "{ACCESS_KEY_FIELD}={}\n{SECRET_KEY_FIELD}={}\n",
        creds.access_key, creds.secret_key
    )
}

/// Parse a file body written by [`render`]. Unknown keys and blank lines are
/// ignored; `None` when either field is missing or empty.
#[must_use]
pub fn parse(text: &str) -> Option<RootCredentials> {
    let mut access_key = None;
    let mut secret_key = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            ACCESS_KEY_FIELD => access_key = Some(value.trim().to_owned()),
            SECRET_KEY_FIELD => secret_key = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    match (access_key, secret_key) {
        (Some(access_key), Some(secret_key))
            if !access_key.is_empty() && !secret_key.is_empty() =>
        {
            Some(RootCredentials {
                access_key,
                secret_key,
            })
        }
        _ => None,
    }
}

fn credentials_error(path: &Path, detail: String) -> ServiceError {
    ServiceError::Credentials {
        path: path.to_path_buf(),
        detail,
    }
}

/// Read `service_id`'s stored pair. `Ok(None)` when no file exists yet; a file
/// that exists but cannot be read or parsed is [`ServiceError::Credentials`].
///
/// # Errors
/// [`ServiceError::Credentials`] for an unreadable or unparsable file.
pub fn load(
    dirs: &PlatformDirs,
    service_id: &str,
) -> Result<Option<RootCredentials>, ServiceError> {
    let path = credentials_path(dirs, service_id);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(credentials_error(&path, format!("read failed: {e}"))),
    };
    parse(&text).map(Some).ok_or_else(|| {
        credentials_error(
            &path,
            "unreadable or incomplete; delete it to generate new keys".to_owned(),
        )
    })
}

/// Load `service_id`'s pair, generating and persisting one when none exists.
///
/// The parent dir is created first (credentials may be read before the first
/// start). The pair is written and synced to a uniquely named temp file opened
/// `create_new` with mode `0o600` on Unix (other platforms get default ACLs),
/// then hard-linked into place. The link fails rather than replacing an
/// existing file, so the credentials path only ever holds a complete pair: a
/// concurrent caller or a crash mid-write never leaves an empty file behind. If
/// another caller won the race, its pair is loaded instead.
///
/// # Errors
/// [`ServiceError::Credentials`] when the stored file is corrupt or the new one
/// cannot be written.
pub fn load_or_create(
    dirs: &PlatformDirs,
    service_id: &str,
) -> Result<RootCredentials, ServiceError> {
    if let Some(creds) = load(dirs, service_id)? {
        return Ok(creds);
    }
    let path = credentials_path(dirs, service_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| credentials_error(&path, format!("create parent dir failed: {e}")))?;
    }
    let creds = generate();
    let tmp = path.with_file_name(format!(
        "credentials.tmp-{}-{}",
        std::process::id(),
        random_alphanumeric(&mut rand::rngs::OsRng, 8)
    ));
    let written = write_new_file(&tmp, render(&creds).as_bytes());
    let linked = written.and_then(|()| std::fs::hard_link(&tmp, &path));
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(creds),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => load(dirs, service_id)?
            .ok_or_else(|| credentials_error(&path, "disappeared while being created".to_owned())),
        Err(e) => Err(credentials_error(&path, format!("write failed: {e}"))),
    }
}

fn write_new_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// Delete `service_id`'s stored pair. A missing file is success.
///
/// # Errors
/// [`ServiceError::Credentials`] when the file exists but cannot be removed.
pub fn remove(dirs: &PlatformDirs, service_id: &str) -> Result<(), ServiceError> {
    let path = credentials_path(dirs, service_id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(credentials_error(&path, format!("remove failed: {e}"))),
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
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    const ID: &str = "versitygw";

    fn dirs_in(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    fn pair(access: &str, secret: &str) -> RootCredentials {
        RootCredentials {
            access_key: access.to_owned(),
            secret_key: secret.to_owned(),
        }
    }

    #[test]
    fn generate_has_the_documented_shape() {
        for seed in [0_u64, 1, 42, u64::MAX] {
            let creds = generate_with(&mut StdRng::seed_from_u64(seed));
            let suffix = creds.access_key.strip_prefix("yerd_").unwrap();
            assert_eq!(suffix.len(), 10, "{seed}");
            assert!(suffix.chars().all(|c| c.is_ascii_alphanumeric()), "{seed}");
            assert_eq!(creds.secret_key.len(), 40, "{seed}");
            assert!(
                creds.secret_key.chars().all(|c| c.is_ascii_alphanumeric()),
                "{seed}"
            );
        }
    }

    #[test]
    fn generate_is_deterministic_per_seed_and_differs_across_seeds() {
        let a = generate_with(&mut StdRng::seed_from_u64(7));
        let b = generate_with(&mut StdRng::seed_from_u64(7));
        let c = generate_with(&mut StdRng::seed_from_u64(8));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(generate(), generate());
    }

    #[test]
    fn render_then_parse_round_trips() {
        let creds = generate_with(&mut StdRng::seed_from_u64(3));
        assert_eq!(parse(&render(&creds)), Some(creds));
    }

    #[test]
    fn render_is_the_two_line_format() {
        assert_eq!(
            render(&pair("yerd_abc", "s3cr3t")),
            "access_key=yerd_abc\nsecret_key=s3cr3t\n"
        );
    }

    #[test]
    fn parse_accepts_unknown_keys_and_blank_lines() {
        let text = "\nfuture=1\naccess_key=yerd_a\n\nsecret_key=b\n";
        assert_eq!(parse(text), Some(pair("yerd_a", "b")));
    }

    #[test]
    fn parse_rejects_incomplete_inputs() {
        let cases = [
            "",
            "garbage",
            "access_key=yerd_a\n",
            "secret_key=b\n",
            "access_key=\nsecret_key=b\n",
            "access_key=yerd_a\nsecret_key=\n",
            "access_key=yerd_a\nsecret_key=   \n",
        ];
        for text in cases {
            assert_eq!(parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn debug_redacts_the_secret() {
        let creds = pair("yerd_visible", "TopSecretValue123");
        let shown = format!("{creds:?}");
        assert!(!shown.contains("TopSecretValue123"), "{shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
        assert!(shown.contains("yerd_visible"), "{shown}");
    }

    #[test]
    fn load_is_none_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load(&dirs_in(tmp.path()), ID).unwrap(), None);
    }

    #[test]
    fn load_or_create_creates_parent_then_reuses_the_same_pair() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let path = credentials_path(&dirs, ID);
        assert!(!path.parent().unwrap().exists());
        let first = load_or_create(&dirs, ID).unwrap();
        assert!(path.is_file());
        let second = load_or_create(&dirs, ID).unwrap();
        assert_eq!(first, second);
        assert_eq!(load(&dirs, ID).unwrap(), Some(first.clone()));
        assert_eq!(parse(&std::fs::read_to_string(&path).unwrap()), Some(first));
    }

    #[cfg(unix)]
    #[test]
    fn created_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        load_or_create(&dirs, ID).unwrap();
        let mode = std::fs::metadata(credentials_path(&dirs, ID))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn corrupt_file_is_an_error_and_left_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let path = credentials_path(&dirs, ID);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "access_key=only-half\n").unwrap();
        for result in [
            load(&dirs, ID).map(|_| ()),
            load_or_create(&dirs, ID).map(|_| ()),
        ] {
            match result {
                Err(ServiceError::Credentials { path: p, detail }) => {
                    assert_eq!(p, path);
                    assert!(detail.contains("delete it"), "{detail}");
                    assert!(!detail.contains("only-half"), "{detail}");
                }
                other => panic!("expected Credentials error, got {other:?}"),
            }
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "access_key=only-half\n"
        );
    }

    #[test]
    fn load_or_create_leaves_no_temp_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        load_or_create(&dirs, ID).unwrap();
        let path = credentials_path(&dirs, ID);
        let names: Vec<String> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["credentials".to_owned()]);
    }

    #[test]
    fn concurrent_load_or_create_agree_on_one_pair() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let results: Vec<RootCredentials> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| s.spawn(|| load_or_create(&dirs, ID).unwrap()))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let stored = load(&dirs, ID).unwrap().unwrap();
        for creds in results {
            assert_eq!(creds, stored);
        }
    }

    #[test]
    fn remove_deletes_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        remove(&dirs, ID).unwrap();
        load_or_create(&dirs, ID).unwrap();
        remove(&dirs, ID).unwrap();
        assert!(!credentials_path(&dirs, ID).exists());
        remove(&dirs, ID).unwrap();
        assert_eq!(load(&dirs, ID).unwrap(), None);
    }

    #[test]
    fn remove_then_create_yields_a_fresh_pair() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let first = load_or_create(&dirs, ID).unwrap();
        remove(&dirs, ID).unwrap();
        let second = load_or_create(&dirs, ID).unwrap();
        assert_ne!(first, second);
    }
}
