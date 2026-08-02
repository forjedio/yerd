//! Embed an `asInvoker` application manifest into the crate's Windows binaries
//! (including the test binary).
//!
//! Windows' UAC "installer detection" heuristic auto-elevates executables whose
//! names contain keywords like "update"/"install"/"setup". The `yerd_update`
//! test binary trips this, so under a non-elevated shell (and potentially a CI
//! runner) it fails to launch with os error 740 ("The requested operation
//! requires elevation"). Declaring `asInvoker` opts out of the heuristic. It is
//! also the correct level for Yerd's self-update path: it never self-elevates.
//!
//! MSVC-only and effect-free everywhere else, so no Unix behaviour changes.

use std::path::Path;

fn main() {
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env != "msvc" {
        return;
    }
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let manifest = Path::new(&manifest_dir).join("yerd-update.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
}
