//! CLI surface (clap-derived).

use std::path::PathBuf;

/// Top-level parser.
#[derive(clap::Parser, Debug)]
#[command(name = "yerdd", version, about = "Yerd daemon")]
pub struct Cli {
    /// Subcommand to run; defaults to `Serve` with default args when omitted.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Daemon subcommands.
#[derive(clap::Subcommand, Debug)]
pub enum Command {
    /// Run the daemon in the foreground.
    Serve(ServeArgs),
}

/// Arguments to the `serve` subcommand.
#[derive(clap::Args, Debug, Default)]
pub struct ServeArgs {
    /// Increase log verbosity. `-v` → debug, `-vv` → trace.
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// Override the config file location.
    #[arg(short, long)]
    pub config: Option<PathBuf>,
}
