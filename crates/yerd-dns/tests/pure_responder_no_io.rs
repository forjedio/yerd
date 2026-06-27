//! Source-byte tripwire: `responder.rs` and `answer.rs` must not name any
//! I/O-shaped path. Smoke check, **not** a purity proof - determined
//! contributors can sneak imports past it (`use std::time as t; t::Instant`).
//! The realistic regression mode is an accidental `use tokio::…` line, and
//! that is caught.
//!
//! The scan includes comments, so the modules under scan must not name the
//! forbidden crates even in commentary.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;

const FORBIDDEN: &[&[u8]] = &[
    b"tokio",
    b"hickory",
    b"std::net",
    b"std::io",
    b"std::fs",
    b"std::env",
    b"std::process",
    b"std::time::Instant",
    b"std::time::SystemTime",
];

const SCANNED: &[&str] = &["src/responder.rs", "src/answer.rs"];

#[test]
fn pure_modules_have_no_io_substrings() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for rel in SCANNED {
        let path = crate_root.join(rel);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
        for needle in FORBIDDEN {
            assert!(
                !contains_subslice(&bytes, needle),
                "{}: forbidden substring {:?} appears (comments are scanned too)",
                path.display(),
                std::str::from_utf8(needle).unwrap()
            );
        }
    }
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}
