//! `yerd` CLI entry point. Parses args, builds a single-threaded tokio
//! runtime, and delegates to [`yerd::run`].

use std::process::ExitCode;

use clap::Parser;

use yerd::cli::Cli;

fn main() -> ExitCode {
    // When invoked under a multi-call alias, act as that shim and exec — before
    // clap ever sees the args. On success `exec` replaces the process; we only
    // get a code back on failure. `composer` runs the bundled phar; the cover
    // aliases (`phpcover` / `php<ver>cover`) run PHP with pcov.
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
    // Hidden applier mode (set by `yerd update --yes` / the GUI). Gated by an env
    // var, parsed from env (not argv), so it never appears in help/completions.
    if let Some(code) = yerd::apply::run_from_env() {
        return code;
    }
    // Hidden elevated deb-installer (re-exec'd under pkexec, which strips env, so
    // this one is argv-gated). Also never a clap subcommand.
    if let Some(code) = yerd::apply::run_install_deb_from_args() {
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
