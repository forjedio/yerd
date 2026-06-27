//! Thin I/O leaves: [`load`] reads a TOML file; [`save`] writes one
//! atomically via write-temp-then-rename.

use std::fs;
use std::path::Path;

use tempfile::NamedTempFile;

use crate::{Config, ConfigError};

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
