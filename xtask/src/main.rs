//! Yerd build automation, invoked as `cargo xtask <command>`.
//!
//! Currently provides `deb` (build a Linux `.deb`). Pure helpers live in
//! [`pack`]; per-command I/O glue lives in its own module (e.g. [`deb`]).

#![forbid(unsafe_code)]

mod deb;
mod pack;

use anyhow::Result;
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Command::Deb(args) => {
            deb::run(args)?;
            Ok(())
        }
    }
}
