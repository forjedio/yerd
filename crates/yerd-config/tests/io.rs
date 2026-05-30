//! Filesystem-backed integration tests for `Config::load` and `Config::save`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;

use tempfile::tempdir;
use yerd_config::{Config, ConfigError, PhpSection};
use yerd_core::PhpVersion;

#[test]
fn save_then_load_round_trip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut original = Config::default();
    original.php = PhpSection {
        default: PhpVersion::new(8, 2),
        settings: std::collections::BTreeMap::new(),
    };
    original.parked.paths.insert("/srv/sites".to_string());
    original.services.enabled.insert("mysql".to_string());
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

    // Write a sentinel first.
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
