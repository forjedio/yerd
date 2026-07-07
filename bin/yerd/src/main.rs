//! `yerd` CLI entry point. Parses args, builds a single-threaded tokio
//! runtime, and delegates to [`yerd::run`].

use std::process::ExitCode;

use clap::Parser;

use yerd::cli::Cli;

fn main() -> ExitCode {
    #[cfg(unix)]
    if let Some(code) = yerd::composer_shim::dispatch() {
        return code;
    }
    #[cfg(unix)]
    if let Some(code) = yerd::cover_shim::dispatch() {
        return code;
    }
    #[cfg(unix)]
    if let Some(code) = yerd::laravel_shim::dispatch() {
        return code;
    }
    #[cfg(unix)]
    if let Some(code) = yerd::wp_shim::dispatch() {
        return code;
    }
    if let Some(code) = yerd::apply::run_from_env() {
        return code;
    }
    if let Some(code) = yerd::apply::run_install_deb_from_args() {
        return code;
    }
    if let Some(code) = yerd::apply::run_install_pacman_from_args() {
        return code;
    }
    if let Some(code) = yerd::apply::run_install_rpm_from_args() {
        return code;
    }

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
