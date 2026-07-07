//! CLI surface (clap-derived).

use std::path::PathBuf;

/// Top-level parser.
#[derive(clap::Parser, Debug)]
#[command(name = "yerdd", version, about = "Yerd daemon")]
pub struct Cli {
    /// Print the build's self-update package format (`deb`/`pacman`/`rpm`) and exit.
    ///
    /// Hidden diagnostic: the release pipeline runs this on the freshly-built Arch
    /// and Fedora `yerdd` to assert it was compiled with the matching
    /// `pacman`/`rpm` feature, so a forgotten `--features` flag fails the release
    /// instead of shipping a `.deb`-format updater inside the `.pkg.tar.zst` or
    /// `.rpm`. Handled in `main` before the daemon starts.
    #[arg(long, hide = true)]
    pub pkg_format: bool,
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
