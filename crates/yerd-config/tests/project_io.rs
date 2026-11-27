//! Filesystem-backed tests for `ProjectFile::load` and `ProjectFile::save`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;

use tempfile::tempdir;
use yerd_config::{ProjectError, ProjectFile, PROJECT_FILE_MAX_BYTES, PROJECT_FILE_NAME};

fn sample() -> ProjectFile {
    ProjectFile::from_toml("name = \"acme\"\nphp = \"8.4\"\nsecure = true\ndomains = [\"*.acme\"]")
        .unwrap()
}

#[test]
fn save_then_load_round_trips() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(PROJECT_FILE_NAME);
    sample().save(&path).unwrap();
    assert_eq!(ProjectFile::load(&path).unwrap(), Some(sample()));
    assert_eq!(fs::read_to_string(&path).unwrap(), sample().to_toml());
}

#[test]
fn save_replaces_an_existing_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(PROJECT_FILE_NAME);
    fs::write(&path, "php = \"8.1\"\n").unwrap();
    sample().save(&path).unwrap();
    assert_eq!(ProjectFile::load(&path).unwrap(), Some(sample()));
}

#[cfg(unix)]
#[test]
fn saved_mode_is_0644() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let path = dir.path().join(PROJECT_FILE_NAME);
    sample().save(&path).unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o644);
}

#[test]
fn save_into_missing_directory_errors() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("missing").join(PROJECT_FILE_NAME);
    assert!(matches!(sample().save(&path), Err(ProjectError::Io { .. })));
    assert!(!dir.path().join("missing").exists());
}

#[test]
fn missing_file_loads_as_none() {
    let dir = tempdir().unwrap();
    assert_eq!(
        ProjectFile::load(&dir.path().join(PROJECT_FILE_NAME)).unwrap(),
        None
    );
}

#[cfg(unix)]
#[test]
fn symlink_is_refused_for_load_and_save() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("real.toml");
    fs::write(&target, "php = \"8.4\"\n").unwrap();
    let link = dir.path().join(PROJECT_FILE_NAME);
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(matches!(
        ProjectFile::load(&link),
        Err(ProjectError::NotRegularFile { .. })
    ));
    assert!(matches!(
        sample().save(&link),
        Err(ProjectError::NotRegularFile { .. })
    ));
    assert_eq!(fs::read_to_string(&target).unwrap(), "php = \"8.4\"\n");
}

#[test]
fn directory_is_refused() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(PROJECT_FILE_NAME);
    fs::create_dir(&path).unwrap();
    assert!(matches!(
        ProjectFile::load(&path),
        Err(ProjectError::NotRegularFile { .. })
    ));
    assert!(matches!(
        sample().save(&path),
        Err(ProjectError::NotRegularFile { .. })
    ));
}

#[test]
fn oversized_file_is_refused() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(PROJECT_FILE_NAME);
    let size = usize::try_from(PROJECT_FILE_MAX_BYTES).unwrap() + 1;
    fs::write(&path, "#".repeat(size)).unwrap();
    assert!(matches!(
        ProjectFile::load(&path),
        Err(ProjectError::TooLarge { .. })
    ));
}

#[test]
fn invalid_contents_surface_a_parse_error() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(PROJECT_FILE_NAME);
    fs::write(&path, "colour = \"blue\"\n").unwrap();
    assert!(matches!(
        ProjectFile::load(&path),
        Err(ProjectError::Parse(_))
    ));
}

#[test]
fn invalid_utf8_is_a_content_error() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(PROJECT_FILE_NAME);
    fs::write(&path, b"name = \"\xff\"\n").unwrap();
    assert!(matches!(
        ProjectFile::load(&path),
        Err(ProjectError::Invalid { field: "file", .. })
    ));
}

fn with_header_value(value: &str) -> ProjectFile {
    ProjectFile {
        headers: vec![yerd_core::ResponseHeader::new("X-Pad", value).unwrap()],
        ..ProjectFile::default()
    }
}

fn sized(len: u64) -> ProjectFile {
    let base = u64::try_from(with_header_value("a").to_toml().len()).unwrap();
    let pad = usize::try_from(len - base + 1).unwrap();
    with_header_value(&"a".repeat(pad))
}

#[test]
fn save_refuses_output_over_the_size_limit() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(PROJECT_FILE_NAME);
    let exact = sized(PROJECT_FILE_MAX_BYTES);
    assert_eq!(
        u64::try_from(exact.to_toml().len()).unwrap(),
        PROJECT_FILE_MAX_BYTES
    );
    exact.save(&path).unwrap();
    assert_eq!(ProjectFile::load(&path).unwrap(), Some(exact.clone()));

    let over = sized(PROJECT_FILE_MAX_BYTES + 1);
    assert!(matches!(
        over.save(&path),
        Err(ProjectError::TooLarge { .. })
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), exact.to_toml());
}

#[test]
fn save_validates_first() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(PROJECT_FILE_NAME);
    let bad = ProjectFile {
        extensions: vec!["Not Valid".to_owned()],
        ..ProjectFile::default()
    };
    assert!(matches!(
        bad.save(&path),
        Err(ProjectError::Invalid {
            field: "extensions",
            ..
        })
    ));
    assert!(!path.exists());
}

#[test]
fn save_new_never_replaces_an_existing_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(PROJECT_FILE_NAME);
    sample().save_new(&path).unwrap();
    assert_eq!(ProjectFile::load(&path).unwrap(), Some(sample()));

    fs::write(&path, "php = \"8.1\"\n").unwrap();
    assert!(matches!(
        sample().save_new(&path),
        Err(ProjectError::AlreadyExists { .. })
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "php = \"8.1\"\n");
}
