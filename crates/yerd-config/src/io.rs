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

pub(crate) fn save(cfg: &Config, path: &Path) -> Result<(), ConfigError> {
    let serialised = cfg.to_toml()?;

    // `path.parent()` is `None` for the empty path; an empty parent (`""`)
    // arises for bare file names. Treat both as CWD-relative.
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

    // `persist` calls `rename(2)` on Unix (atomic when src and dst are on
    // the same filesystem — guaranteed: we created the temp file in
    // `parent`). On Windows it calls `MoveFileExW` with
    // `MOVEFILE_REPLACE_EXISTING`, which is atomic for the rename itself
    // but can fail with `ERROR_SHARING_VIOLATION` if another process holds
    // an exclusive handle to the destination. The daemon must not hold a
    // write handle to the config file between save calls.
    //
    // On failure, `persist` returns a `PersistError` carrying the original
    // `NamedTempFile`; we drop it, which deletes the temp file via
    // `NamedTempFile::Drop`. No orphan tmp files are left behind.
    //
    // Unix mode: `NamedTempFile` creates the file with mode 0600 (owner
    // read/write only). This propagates to the destination on `persist`.
    // Intentional — the daemon is the only writer; broader permissions are
    // the operator's call to set after install.
    //
    // No `fsync` of file or parent dir: portability cost (Windows lacks an
    // exact equivalent) outweighs durability gain for a developer-only
    // config file. Loss under sudden power loss is acceptable.
    tmp.persist(path).map_err(|e| ConfigError::Io {
        path: path.to_path_buf(),
        source: e.error,
    })?;
    Ok(())
}
