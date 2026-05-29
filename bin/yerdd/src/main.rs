//! `yerdd` entry point. Parses CLI args, installs tracing, runs the
//! tokio runtime, and translates `DaemonError` into a sysexits-style
//! exit code.

use std::process::ExitCode;

use clap::Parser;

use yerdd::args::{Cli, Command, ServeArgs};
use yerdd::{error, run, tracing_init};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Command::Serve(args) = cli
        .command
        .unwrap_or_else(|| Command::Serve(ServeArgs::default()));
    tracing_init::init(args.verbose);

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("yerdd: cannot build tokio runtime: {e}");
            return ExitCode::from(70);
        }
    };

    match runtime.block_on(run(args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "yerdd exiting with error");
            ExitCode::from(error::exit_code(&e))
        }
    }
}
