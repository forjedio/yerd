//! Filesystem-backed integration tests for `Config::load` and `Config::save`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;

use tempfile::tempdir;
use yerd_config::{Config, ConfigError, PhpSection, ServiceInstance};
use yerd_core::PhpVersion;

#[test]
fn save_then_load_round_trip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut original = Config::default();
    original.php = PhpSection {
        default: PhpVersion::new(8, 2),
        settings: std::collections::BTreeMap::new(),
        extensions: std::collections::BTreeMap::new(),
    };
    original.parked.paths.insert("/srv/sites".to_string());
    original
        .services
        .instances
        .insert("mysql".to_string(), ServiceInstance::default());
    original.save(&path).unwrap();

    let loaded = Config::load(&path).unwrap();
    assert_eq!(loaded, original);
}

#[test]
fn save_creates_parent_dirs() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("nested").join("dir").join("config.toml");

    Config::default().save(&path).unwrap();
    assert!(path.exists(), "save should have created the file");
}

#[test]
fn load_missing_file_returns_io_error_with_requested_path() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("does-not-exist.toml");

    match Config::load(&path) {
        Err(ConfigError::Io { path: p, .. }) => {
            assert_eq!(p, path, "Io error must carry the caller-supplied path");
        }
        other => panic!("expected Io error, got {other:?}"),
    }
}

#[test]
fn save_overwrites_existing_with_new_content() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    fs::write(&path, "sentinel = 0\n").unwrap();

    let mut new = Config::default();
    new.parked.paths.insert("/srv/replaced".to_string());
    new.save(&path).unwrap();

    let loaded = Config::load(&path).unwrap();
    assert_eq!(loaded, new);
    let raw = fs::read_to_string(&path).unwrap();
    assert!(!raw.contains("sentinel"));
}

#[test]
fn load_invalid_toml_returns_parse_error() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad.toml");
    fs::write(&path, "&^%$").unwrap();

    assert!(matches!(Config::load(&path), Err(ConfigError::Parse(_))));
}

/// A regular file sits where `save` tries to materialise a parent directory, so
/// `create_dir_all(<file>/sub)` fails with ENOTDIR, exercising the
/// `create_dir_all` error arm of `save`.
#[test]
fn save_create_dir_all_failure_returns_io_error_with_requested_path() {
    let dir = tempdir().unwrap();
    let blocker = dir.path().join("not-a-dir");
    fs::write(&blocker, b"i am a file, not a directory").unwrap();
    let path = blocker.join("sub").join("config.toml");

    match Config::default().save(&path) {
        Err(ConfigError::Io { path: p, .. }) => {
            assert_eq!(p, path, "Io error must carry the caller-supplied path");
        }
        other => panic!("expected Io error from create_dir_all, got {other:?}"),
    }
}

/// The destination path already exists as a directory, so `create_dir_all` is a
/// no-op and the temp file writes fine, but `persist`'s rename onto a directory
/// fails, exercising the `persist` error arm of `save`.
#[test]
fn save_persist_onto_directory_returns_io_error_with_requested_path() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::create_dir(&path).unwrap();

    match Config::default().save(&path) {
        Err(ConfigError::Io { path: p, .. }) => {
            assert_eq!(p, path, "Io error must carry the caller-supplied path");
        }
        other => panic!("expected Io error from persist, got {other:?}"),
    }
    assert!(
        path.is_dir(),
        "destination directory must survive a failed save"
    );
}

/// A pre-existing but read-only parent makes `create_dir_all` a no-op while
/// `NamedTempFile::new_in` cannot create the temp file, exercising the
/// temp-file-creation error arm of `save`. Skipped when the process can write
/// regardless of mode bits (e.g. root in CI); permissions are restored before
/// asserting so the tempdir cleanup runs.
#[cfg(unix)]
#[test]
fn save_into_readonly_parent_returns_io_error() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let ro = dir.path().join("readonly");
    fs::create_dir(&ro).unwrap();
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o500)).unwrap();

    let writable = fs::File::create(ro.join(".probe")).is_ok();
    let _ = fs::remove_file(ro.join(".probe"));
    if writable {
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o700)).unwrap();
        return;
    }

    let path = ro.join("config.toml");
    let result = Config::default().save(&path);

    fs::set_permissions(&ro, fs::Permissions::from_mode(0o700)).unwrap();

    match result {
        Err(ConfigError::Io { path: p, .. }) => {
            assert_eq!(p, path, "Io error must carry the caller-supplied path");
        }
        other => panic!("expected Io error from temp-file creation, got {other:?}"),
    }
}
