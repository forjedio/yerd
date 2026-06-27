//! Isolated single-test binary for the CWD-mutating parent-less-path case.
//!
//! `cargo test` runs `#[test]` functions within the same integration-test
//! binary in parallel. Mutating the process's CWD races with peer tests.
//! This file holds exactly one test so it has the binary to itself.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::env;
use std::path::Path;

use tempfile::tempdir;
use yerd_config::Config;

#[test]
fn save_treats_parentless_path_as_cwd() {
    let dir = tempdir().unwrap();
    let prev_cwd = env::current_dir().unwrap();
    env::set_current_dir(dir.path()).unwrap();

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        Config::default().save(Path::new("yerd-test.toml"))?;
        let in_cwd = dir.path().join("yerd-test.toml");
        assert!(in_cwd.exists(), "expected file in CWD: {in_cwd:?}");
        Ok(())
    })();

    env::set_current_dir(prev_cwd).unwrap();
    result.unwrap();
}
