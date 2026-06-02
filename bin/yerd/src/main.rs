//! `yerd` CLI entry point. Parses args, builds a single-threaded tokio
//! runtime, and delegates to [`yerd::run`].

use std::process::ExitCode;

use clap::Parser;

use yerd::cli::Cli;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("yerd: cannot build tokio runtime: {e}");
            return ExitCode::from(70);
        }
    };
    runtime.block_on(yerd::run(cli))
}
