//! `node`, `npm`, `npx`, `pnpm`, `pnpx`, `bun` and `bunx` multi-call shims.
//!
//! `{data}/bin/<name>` is a symlink to *this* `yerd` binary. Invoked under one
//! of those names, yerd resolves the same target the daemon used to link
//! directly (the global default Node's `bin/<name>`, or the pnpm/Bun root),
//! fills in the outbound proxy environment from `{data}/network.env` when the
//! caller has none, then `exec`s the real binary with `argv[0]` preserved
//! (pnpx and bunx dispatch on it). Unix-only.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use yerd_platform::{ActivePaths, Paths, PlatformDirs};

use crate::shim::fail;

/// Which managed tool a shim name belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetKind {
    /// `node`, `npm`, `npx`: the global default Node release.
    Node,
    /// `pnpm`, `pnpx`: the pnpm standalone root.
    Pnpm,
    /// `bun`, `bunx`: the Bun binary.
    Bun,
}

/// The tool a shim name dispatches to, or `None` for any other name.
#[must_use]
pub(crate) fn target_kind(name: &str) -> Option<TargetKind> {
    match name {
        "node" | "npm" | "npx" => Some(TargetKind::Node),
        "pnpm" | "pnpx" => Some(TargetKind::Pnpm),
        "bun" | "bunx" => Some(TargetKind::Bun),
        _ => None,
    }
}

/// The real executable for shim `name`. A missing or unreadable `yerd.toml`
/// never blocks Node: it falls back to the legacy-install default, as the daemon
/// does when no default is configured.
///
/// # Errors
/// A message when the tool, or the Node default, is not installed.
pub(crate) fn resolve_target(
    dirs: &PlatformDirs,
    name: &str,
    kind: TargetKind,
) -> Result<PathBuf, String> {
    let root = |tool: &str| {
        yerd_platform::single_top_level_dir(&dirs.data.join("tools").join(tool))
            .map_err(|e| e.to_string())
    };
    match kind {
        TargetKind::Node => {
            let configured = yerd_config::Config::load(&dirs.config.join("yerd.toml"))
                .ok()
                .and_then(|c| c.node.default);
            let selector = yerd_platform::node::default_selector(dirs, configured.as_deref())
                .ok_or_else(|| "no default Node version is set".to_owned())?;
            let installed = yerd_platform::node::resolve(dirs, &selector)?;
            Ok(installed.bin.join(name))
        }
        TargetKind::Pnpm => Ok(root("pnpm")?.join(name)),
        TargetKind::Bun => Ok(root("bun")?.join("bun")),
    }
}

/// If `argv[0]` names a Node, pnpm, or Bun command, exec the managed tool and
/// return its exit code on failure (on success `exec` never returns);
/// otherwise `None`, so `main` falls through to the next shim / CLI.
#[must_use]
pub fn dispatch() -> Option<ExitCode> {
    let arg0 = std::env::args_os().next()?;
    let name = Path::new(&arg0).file_name()?.to_str()?.to_owned();
    let kind = target_kind(&name)?;
    Some(run(&name, kind))
}

fn run(name: &str, kind: TargetKind) -> ExitCode {
    let dirs = match ActivePaths::new().resolve() {
        Ok(d) => d,
        Err(e) => return fail(format!("cannot resolve yerd directories: {e}")),
    };
    let target = match resolve_target(&dirs, name, kind) {
        Ok(t) => t,
        Err(e) => {
            return fail(format!(
                "{name} is not available ({e}) — reinstall it from the Tooling page"
            ))
        }
    };
    let mut cmd = Command::new(&target);
    cmd.arg0(name).args(std::env::args_os().skip(1));
    crate::shim::apply_network_env(&mut cmd, &dirs);
    let err = cmd.exec();
    fail(format!(
        "failed to exec {} ({err}) — reinstall {name} from the Tooling page",
        target.display()
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn target_kind_table() {
        for (name, kind) in [
            ("node", Some(TargetKind::Node)),
            ("npm", Some(TargetKind::Node)),
            ("npx", Some(TargetKind::Node)),
            ("pnpm", Some(TargetKind::Pnpm)),
            ("pnpx", Some(TargetKind::Pnpm)),
            ("bun", Some(TargetKind::Bun)),
            ("bunx", Some(TargetKind::Bun)),
            ("composer", None),
            ("php", None),
            ("nodejs", None),
            ("yerd", None),
        ] {
            assert_eq!(target_kind(name), kind, "{name}");
        }
    }

    fn dirs_at(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    #[test]
    fn resolves_the_targets_the_daemon_used_to_link() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        let node_root = dirs.data.join("tools/node-versions/24.2.0");
        let dist_bin = node_root.join("node-v24.2.0-darwin-arm64/bin");
        for n in ["node", "npm", "npx"] {
            touch(&dist_bin.join(n));
        }
        std::fs::write(node_root.join(".version"), "v24.2.0").unwrap();
        std::fs::create_dir_all(&dirs.config).unwrap();
        std::fs::write(
            dirs.config.join("yerd.toml"),
            "version = 28\n[node]\ndefault = \"24.2.0\"\n",
        )
        .unwrap();
        let pnpm_root = dirs.data.join("tools/pnpm/pnpm-10.0.0");
        touch(&pnpm_root.join("pnpm"));
        touch(&pnpm_root.join("pnpx"));
        let bun_root = dirs.data.join("tools/bun/bun-darwin-aarch64");
        touch(&bun_root.join("bun"));

        for (name, expected) in [
            ("node", dist_bin.join("node")),
            ("npm", dist_bin.join("npm")),
            ("npx", dist_bin.join("npx")),
            ("pnpm", pnpm_root.join("pnpm")),
            ("pnpx", pnpm_root.join("pnpx")),
            ("bun", bun_root.join("bun")),
            ("bunx", bun_root.join("bun")),
        ] {
            let kind = target_kind(name).unwrap();
            assert_eq!(
                resolve_target(&dirs, name, kind).unwrap(),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn node_falls_back_to_the_legacy_default_when_config_is_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        let legacy = dirs.data.join("tools/node");
        let dist_bin = legacy.join("node-v22.9.0-linux-x64/bin");
        for n in ["node", "npm", "npx"] {
            touch(&dist_bin.join(n));
        }
        std::fs::write(legacy.join(".version"), "v22.9.0").unwrap();
        std::fs::create_dir_all(&dirs.config).unwrap();
        std::fs::write(dirs.config.join("yerd.toml"), "not = [valid").unwrap();
        assert_eq!(
            resolve_target(&dirs, "npm", TargetKind::Node).unwrap(),
            dist_bin.join("npm")
        );
    }

    #[test]
    fn missing_tools_are_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        for name in ["node", "pnpm", "bunx"] {
            assert!(resolve_target(&dirs, name, target_kind(name).unwrap()).is_err());
        }
    }
}
