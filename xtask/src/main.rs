//! Yerd build automation, invoked as `cargo xtask <command>`.
//!
//! Provides `deb` (build a Linux `.deb`), `bump` (set the project version across
//! the three manifests), and `version-check` (assert a tag matches them). Pure
//! helpers live in [`pack`] / [`version`]; per-command I/O glue lives here or in
//! the command's module (e.g. [`deb`]).

#![forbid(unsafe_code)]

mod deb;
mod pack;
mod version;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;

/// Top-level `xtask` command-line parser.
#[derive(Parser, Debug)]
#[command(name = "xtask", about = "Yerd build automation")]
pub struct Cli {
    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// `xtask` subcommands.
#[derive(clap::Subcommand, Debug)]
pub enum Command {
    /// Build a Linux `.deb` package for the Yerd binaries.
    Deb(deb::DebArgs),
    /// Set the project version across Cargo.toml, tauri.conf.json, package.json.
    Bump {
        /// The new version, e.g. `2.0.2` or `2.0.2-rc.1` (a leading `v` is fine).
        version: String,
    },
    /// Assert the given tag/version matches all three manifests (release gate).
    VersionCheck {
        /// The tag/version to check, e.g. `v2.0.2` (a leading `v` is stripped).
        version: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Command::Deb(args) => {
            deb::run(args)?;
            Ok(())
        }
        Command::Bump { version } => run_bump(version),
        Command::VersionCheck { version } => run_version_check(version),
    }
}

/// The three manifests whose `version` must stay in sync.
struct Manifests {
    cargo: PathBuf,
    tauri: PathBuf,
    package_json: PathBuf,
}

impl Manifests {
    fn locate() -> Self {
        // xtask lives at <root>/xtask, so the workspace root is its parent.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Self {
            cargo: root.join("Cargo.toml"),
            tauri: root.join("apps/yerd-gui/src-tauri/tauri.conf.json"),
            package_json: root.join("apps/yerd-gui/package.json"),
        }
    }
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

fn run_bump(raw: &str) -> Result<()> {
    let version = version::normalise(raw);
    let m = Manifests::locate();

    let cargo = version::set_cargo(&read(&m.cargo)?, version)?;
    let tauri = version::set_json(&read(&m.tauri)?, version)?;
    let pkg = version::set_json(&read(&m.package_json)?, version)?;

    fs::write(&m.cargo, cargo).with_context(|| format!("writing {}", m.cargo.display()))?;
    fs::write(&m.tauri, tauri).with_context(|| format!("writing {}", m.tauri.display()))?;
    fs::write(&m.package_json, pkg)
        .with_context(|| format!("writing {}", m.package_json.display()))?;

    println!("Bumped version to {version} in:");
    println!("  {}", m.cargo.display());
    println!("  {}", m.tauri.display());
    println!("  {}", m.package_json.display());
    println!("Commit the change, then tag `v{version}`.");
    Ok(())
}

fn run_version_check(raw: &str) -> Result<()> {
    let expected = version::normalise(raw);
    let m = Manifests::locate();

    let found = [
        version::Found {
            label: "Cargo.toml",
            version: version::get_cargo(&read(&m.cargo)?)?,
        },
        version::Found {
            label: "tauri.conf.json",
            version: version::get_json(&read(&m.tauri)?)?,
        },
        version::Found {
            label: "package.json",
            version: version::get_json(&read(&m.package_json)?)?,
        },
    ];

    version::assert_all_match(expected, &found)?;
    println!("OK: all manifests are at {expected}");
    Ok(())
}
