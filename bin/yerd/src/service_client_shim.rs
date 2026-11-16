//! Service client multi-call shim (`mysql`, `pg_dump`, `valkey-cli`, ...).
//!
//! The daemon links each installed engine's curated client names in
//! `{data}/bin` to *this* `yerd` binary. When invoked under one of those names
//! (detected from `argv[0]` before clap), yerd picks the version the daemon runs
//! (the configured one if installed, else the newest), adds connection defaults
//! the user has not set (see [`yerd_services::client::connection_plan`]), and
//! `exec`s the real client from that version's install. No daemon is needed.
//! Unix-only.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::sync::Arc;

use yerd_platform::{ActivePaths, Paths};
use yerd_services::{client, version, ClientDefaults, ServiceDefinition, ServiceRegistry};

use crate::shim::fail;

/// If `argv[0]` is an allowlisted service client name, exec that client from
/// the version Yerd runs and return its exit code (on success `exec` replaces
/// the process and never returns); otherwise `None`, so `main` falls through to
/// the next shim / CLI.
#[must_use]
pub fn dispatch() -> Option<ExitCode> {
    let arg0 = std::env::args_os().next()?;
    let name = Path::new(&arg0).file_name()?.to_str()?;
    let registry = ServiceRegistry::builtin();
    let def = client::owning_engine(&registry, name)?;
    let defaults = def.client_tools()?.defaults;
    Some(run(&registry, &def, name, defaults))
}

fn run(
    registry: &ServiceRegistry,
    def: &Arc<dyn ServiceDefinition>,
    name: &str,
    defaults: ClientDefaults,
) -> ExitCode {
    let dirs = match ActivePaths::new().resolve() {
        Ok(d) => d,
        Err(e) => return fail(format!("cannot resolve yerd directories: {e}")),
    };

    let instance = yerd_config::Config::load(&dirs.config.join("yerd.toml"))
        .ok()
        .and_then(|cfg| cfg.services.instances.get(def.id()).cloned());
    let configured = instance.as_ref().and_then(|i| i.version.clone());
    let port = instance
        .as_ref()
        .and_then(|i| i.port)
        .unwrap_or(def.default_port());

    let installed = version::discover_installed(&dirs, registry)
        .ok()
        .and_then(|mut m| m.remove(def.id()))
        .unwrap_or_default();
    let Some(v) = client::select_version(installed, configured.as_deref()) else {
        return fail(format!(
            "{} is not installed - run `yerd service install {} <version>` \
             (see `yerd service available`)",
            def.display_name(),
            def.id()
        ));
    };

    let bin = version::install_dir(&dirs, def.id(), &v)
        .join("bin")
        .join(name);
    if !bin.is_file() {
        return fail(format!(
            "`{name}` is not included in the installed {} {v} build",
            def.display_name()
        ));
    }

    let forward: Vec<OsString> = std::env::args_os().skip(1).collect();
    let plan = client::connection_plan(
        defaults,
        port,
        &version::socket_path(&dirs, def.id()),
        &forward,
        &|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()),
    );

    let err = Command::new(&bin)
        .envs(plan.env)
        .args(plan.leading_args)
        .args(forward)
        .exec();
    fail(format!("failed to exec {}: {err}", bin.display()))
}
