//! Source invariant: the daemon never spawns a child through a raw
//! `Command::new`.
//!
//! `yerdd` runs without a console on Windows, so a console-subsystem child
//! (php.exe, the database clients, git, cloudflared) spawned without
//! `CREATE_NO_WINDOW` makes the OS allocate a fresh console *window* for it -
//! the console flashes users see when Yerd starts or runs a tool. The flag is
//! applied in exactly two seams: `yerd-supervise`'s `TokioProcessSpawner` for
//! supervised children, and [`yerdd::spawn::hidden_command`] for the daemon's
//! one-shots. Nothing in the compiler notices a new call site that skips both,
//! so this test does.
//!
//! Grep-based on purpose: the point is to catch a *newly written* raw spawn, and
//! text is what a new call site is. The allowlist below names every file that is
//! legitimately exempt, with the reason.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

/// Files allowed to call `Command::new` directly, and why.
///
/// Paths are relative to `bin/yerdd/src`, with `/` separators.
const ALLOWED: &[(&str, &str)] = &[
    (
        "spawn.rs",
        "the seam itself - it is what applies CREATE_NO_WINDOW",
    ),
    (
        "main.rs",
        "the restart re-exec: `exec` on Unix, and a Windows spawn that sets \
         CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP inline",
    ),
    (
        "secure_fs.rs",
        "a test-only `icacls` read; the production path uses \
         yerd_platform::hidden_command",
    ),
    (
        "tools/external.rs",
        "a Unix-only login-shell PATH probe (#[cfg(unix)]); Windows reads the \
         registry instead and spawns nothing",
    ),
];

/// Every `.rs` file under `dir`, recursively, as `(relative path, contents)`.
fn rust_sources(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).expect("src dir is readable") {
        let path = entry.expect("readable dir entry").path();
        if path.is_dir() {
            rust_sources(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let rel = path
                .strip_prefix(root)
                .expect("path is under root")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((
                rel,
                std::fs::read_to_string(&path).expect("readable source"),
            ));
        }
    }
}

#[test]
fn every_child_process_is_spawned_through_a_hidden_seam() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&src, &src, &mut sources);
    assert!(
        !sources.is_empty(),
        "found no sources under {}",
        src.display()
    );

    let offenders: Vec<String> = sources
        .iter()
        .filter(|(rel, _)| !ALLOWED.iter().any(|(allowed, _)| allowed == rel))
        .flat_map(|(rel, body)| {
            body.lines()
                .enumerate()
                .filter(|(_, line)| line.contains("Command::new("))
                .map(move |(i, line)| format!("{rel}:{}: {}", i + 1, line.trim()))
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "these spawn a child directly instead of through \
         `crate::spawn::hidden_command` (or the supervisor's spawner), which \
         flashes a console window on Windows:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn the_allowlist_names_only_files_that_exist_and_still_spawn() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    for (rel, reason) in ALLOWED {
        let path = src.join(rel);
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("allowlisted {rel} is unreadable ({e}); drop the entry"));
        assert!(
            body.contains("Command::new("),
            "allowlisted {rel} no longer spawns anything ({reason}); drop the entry"
        );
    }
}
