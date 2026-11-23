//! Thin I/O leaves: [`load`] reads a TOML file; [`save`] writes one
//! atomically via write-temp-then-rename. [`load_project`] and
//! [`save_project`] do the same for a `.yerd.toml` project file.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use tempfile::NamedTempFile;

use crate::{Config, ConfigError, ProjectError, ProjectFile, PROJECT_FILE_MAX_BYTES};

pub(crate) fn load(path: &Path) -> Result<Config, ConfigError> {
    let s = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Config::from_toml(&s)
}

/// Atomically replaces the config file.
///
/// `persist` does an atomic rename on the same filesystem (Unix) or
/// `MoveFileExW` (Windows), which can fail with `ERROR_SHARING_VIOLATION` if
/// another process holds the dest open, so the daemon must not keep a write
/// handle between saves. The temp file is mode 0600 (the daemon is the only
/// writer). No fsync: durability isn't worth the Windows portability cost for
/// a dev-only config.
pub(crate) fn save(cfg: &Config, path: &Path) -> Result<(), ConfigError> {
    let serialised = cfg.to_toml()?;

    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    let tmp = NamedTempFile::new_in(parent).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    fs::write(tmp.path(), serialised.as_bytes()).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    tmp.persist(path).map_err(|e| ConfigError::Io {
        path: path.to_path_buf(),
        source: e.error,
    })?;
    Ok(())
}

/// Reads a project file without following a symlink.
///
/// The path is checked with `symlink_metadata` first, then the opened handle is
/// checked again (and, on Unix, matched by device and inode) so a swap between
/// the check and the open is refused rather than followed.
pub(crate) fn load_project(path: &Path) -> Result<Option<ProjectFile>, ProjectError> {
    let io_err = |source: io::Error| ProjectError::Io {
        path: path.to_path_buf(),
        source,
    };
    let before = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err(e)),
    };
    if !before.file_type().is_file() {
        return Err(ProjectError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    if before.len() > PROJECT_FILE_MAX_BYTES {
        return Err(ProjectError::TooLarge {
            path: path.to_path_buf(),
        });
    }
    let file = fs::File::open(path).map_err(io_err)?;
    let opened = file.metadata().map_err(io_err)?;
    if !opened.is_file() || !same_file(&before, &opened) {
        return Err(ProjectError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    let mut bytes = Vec::new();
    file.take(PROJECT_FILE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_err)?;
    if u64::try_from(bytes.len()).map_or(true, |n| n > PROJECT_FILE_MAX_BYTES) {
        return Err(ProjectError::TooLarge {
            path: path.to_path_buf(),
        });
    }
    let text = String::from_utf8(bytes).map_err(|e| ProjectError::Invalid {
        field: "file",
        reason: format!("not valid UTF-8: {e}"),
    })?;
    ProjectFile::from_toml(&text).map(Some)
}

#[cfg(unix)]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(_a: &fs::Metadata, _b: &fs::Metadata) -> bool {
    true
}

/// Atomically writes a project file with a fixed mode (0644 on Unix), since it
/// is committed and read by other tools. Validates and renders first, refusing
/// output over [`PROJECT_FILE_MAX_BYTES`] before the destination is touched.
/// Does not create the directory. With `replace` false an existing file is
/// kept and [`ProjectError::AlreadyExists`] returned, even one created after
/// the initial check.
pub(crate) fn save_project(
    file: &ProjectFile,
    path: &Path,
    replace: bool,
) -> Result<(), ProjectError> {
    let io_err = |source: io::Error| ProjectError::Io {
        path: path.to_path_buf(),
        source,
    };
    file.validate()?;
    let rendered = file.to_toml();
    if u64::try_from(rendered.len()).map_or(true, |n| n > PROJECT_FILE_MAX_BYTES) {
        return Err(ProjectError::TooLarge {
            path: path.to_path_buf(),
        });
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(ProjectError::NotRegularFile {
                path: path.to_path_buf(),
            });
        }
        Ok(_) if !replace => {
            return Err(ProjectError::AlreadyExists {
                path: path.to_path_buf(),
            });
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(e)),
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let tmp = NamedTempFile::new_in(parent).map_err(io_err)?;
    fs::write(tmp.path(), rendered.as_bytes()).map_err(io_err)?;
    set_public_mode(tmp.path()).map_err(io_err)?;
    let persisted = if replace {
        tmp.persist(path)
    } else {
        tmp.persist_noclobber(path)
    };
    persisted.map_err(|e| match e.error.kind() {
        io::ErrorKind::AlreadyExists => ProjectError::AlreadyExists {
            path: path.to_path_buf(),
        },
        _ => io_err(e.error),
    })?;
    Ok(())
}

#[cfg(unix)]
fn set_public_mode(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o644))
}

#[cfg(not(unix))]
fn set_public_mode(_path: &Path) -> io::Result<()> {
    Ok(())
}
