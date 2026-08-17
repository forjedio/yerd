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
    /// Hidden, Windows-only. The HKCU `Run` autostart entry launches
    /// `serve --detach`; the process immediately respawns itself hidden (no
    /// console window) without `--detach` and exits, so the login entry doesn't
    /// pin a visible console for the daemon's whole lifetime. See `main.rs`
    /// `relaunch_detached`.
    #[cfg(windows)]
    #[arg(long, hide = true)]
    pub detach: bool,
}

/// The argv (after the binary path) to re-run `serve` with when respawning a
/// detached daemon: `serve`, the verbosity re-rendered as repeated `-v`, and the
/// config override if one was given. It deliberately never carries `--detach`, so
/// the hidden child can't loop back into another detach. Pure and table-tested.
#[cfg(windows)]
#[must_use]
pub fn respawn_args(args: &ServeArgs) -> Vec<std::ffi::OsString> {
    use std::ffi::OsString;

    let mut out: Vec<OsString> = vec!["serve".into()];
    for _ in 0..args.verbose {
        out.push("-v".into());
    }
    if let Some(cfg) = &args.config {
        out.push("--config".into());
        out.push(cfg.clone().into_os_string());
    }
    out
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn as_strings(v: &[OsString]) -> Vec<String> {
        v.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn respawn_args_strips_detach_and_rerenders_flags() {
        let args = ServeArgs {
            verbose: 2,
            config: Some(PathBuf::from(r"C:\Users\a b\yerd.toml")),
            detach: true,
        };
        assert_eq!(
            as_strings(&respawn_args(&args)),
            vec!["serve", "-v", "-v", "--config", r"C:\Users\a b\yerd.toml"]
        );
    }

    #[test]
    fn respawn_args_defaults_to_bare_serve() {
        assert_eq!(
            as_strings(&respawn_args(&ServeArgs::default())),
            vec!["serve"]
        );
    }

    #[test]
    fn respawn_args_never_contains_detach() {
        let args = ServeArgs {
            verbose: 0,
            config: None,
            detach: true,
        };
        assert!(respawn_args(&args)
            .iter()
            .all(|a| a.to_string_lossy() != "--detach"));
    }
}
