//! Project-aware Node execution. Bare managed commands use the global default.
use std::ffi::OsString;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::Duration;

use yerd_config::Config;
use yerd_ipc::{Request, Response};
use yerd_platform::{node, ActivePaths, Paths, PlatformDirs};

/// A resolved managed release and its source.
#[derive(Debug)]
pub struct NodeSelection {
    /// Managed release and binaries.
    pub installed: node::InstalledNode,
    /// Registered site name, if matched.
    pub site: Option<String>,
    /// `nvmrc`, `site`, or `default`.
    pub source: &'static str,
}

fn report(message: &str, code: u8) -> ExitCode {
    eprintln!("yerd: {message}");
    ExitCode::from(code)
}

/// Find the nearest `.nvmrc`, stopping at a registered project root.
///
/// # Errors
/// An existing file that cannot be read, is empty or is unsupported must fail.
pub fn nvmrc(cwd: &Path, root: Option<&Path>) -> Result<Option<String>, String> {
    for dir in cwd.ancestors() {
        let file = dir.join(".nvmrc");
        match std::fs::symlink_metadata(&file) {
            Ok(_) => {
                let value = std::fs::read_to_string(&file)
                    .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
                let value = value.trim();
                if !matches!(value, "node" | "lts/*") {
                    yerd_core::node::numeric_selector(value)
                        .map_err(|e| format!("invalid {}: {e}", file.display()))?;
                }
                return Ok(Some(value.to_owned()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot inspect {}: {e}", file.display())),
        }
        if root == Some(dir) {
            break;
        }
    }
    Ok(None)
}

/// Select a release after the site lookup. All selector sources fail explicitly.
///
/// # Errors
/// Invalid project files, missing defaults, and uninstalled versions are errors.
pub fn select(
    dirs: &PlatformDirs,
    config: &Config,
    cwd: &Path,
    site: Option<(&str, &Path)>,
) -> Result<NodeSelection, String> {
    let root = site.map(|(_, root)| root);
    let file = nvmrc(cwd, root)?;
    let preference = root.and_then(|root| config.node.sites.get(root.to_string_lossy().as_ref()));
    let fallback = config
        .node
        .default
        .clone()
        .or_else(|| node::legacy_default(dirs));
    let (selector, source) = if let Some(selector) = file.as_deref() {
        (selector, "nvmrc")
    } else if let Some(selector) = preference {
        (selector.as_str(), "site")
    } else {
        (fallback.as_deref().ok_or_else(|| "no global Node default — run `yerd install tool node` or `yerd node use <version>`".to_owned())?, "default")
    };
    Ok(NodeSelection {
        installed: node::resolve(dirs, selector)?,
        site: site.map(|(name, _)| name.to_owned()),
        source,
    })
}

async fn resolve(site: Option<&str>) -> Result<NodeSelection, (String, u8)> {
    let dirs = ActivePaths::new()
        .resolve()
        .map_err(|e| (e.to_string(), 74))?;
    let config = Config::load(&dirs.config.join("yerd.toml")).map_err(|e| (e.to_string(), 74))?;
    let cwd = std::env::current_dir()
        .and_then(std::fs::canonicalize)
        .map_err(|e| (e.to_string(), 74))?;
    let response = tokio::time::timeout(
        Duration::from_millis(300),
        crate::transport::exchange(&Request::ListSites),
    )
    .await;
    let Ok(Ok(Response::Sites { sites })) = response else {
        return Err((
            "cannot reach the yerd daemon to resolve the Node project — is it running?".to_owned(),
            69,
        ));
    };
    let candidates: Vec<_> = sites
        .iter()
        .filter_map(|entry| {
            let root = std::fs::canonicalize(entry.site.document_root()).ok()?;
            Some((entry.site.name().to_owned(), root))
        })
        .collect();
    let matched = if let Some(name) = site {
        Some(
            candidates
                .iter()
                .find(|(n, _)| *n == name.to_lowercase())
                .ok_or_else(|| (format!("no site named '{name}' — run `yerd sites`"), 2))?,
        )
    } else {
        candidates
            .iter()
            .filter(|(_, root)| cwd.starts_with(root))
            .max_by_key(|(_, root)| root.components().count())
    };
    let search = if site.is_some() {
        matched.map_or(cwd.as_path(), |(_, root)| root.as_path())
    } else {
        &cwd
    };
    select(
        &dirs,
        &config,
        search,
        matched.map(|(name, root)| (name.as_str(), root.as_path())),
    )
    .map_err(|e| (e, 2))
}

/// Build the executable command. npm/npx run their script through the selected
/// Node interpreter; PATH also selects this same Node for child processes.
///
/// # Errors
/// Missing bundled entry points or an invalid PATH are errors.
pub fn command(
    selection: &NodeSelection,
    tool: &str,
    args: &[OsString],
) -> Result<Command, String> {
    let bin = &selection.installed.bin;
    let executable = bin.join(tool);
    if !executable.is_file() {
        return Err(format!(
            "missing managed {tool} at {} — reinstall Node {}",
            executable.display(),
            selection.installed.version
        ));
    }
    let mut cmd = Command::new(bin.join("node"));
    if tool != "node" {
        cmd.arg(executable);
    }
    cmd.args(args);
    let mut paths = vec![bin.clone()];
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    cmd.env(
        "PATH",
        std::env::join_paths(paths).map_err(|e| e.to_string())?,
    );
    Ok(cmd)
}

/// Run a managed tool, replacing the CLI process on success.
pub async fn run_exec(tool: &str, site: Option<&str>, args: &[OsString]) -> ExitCode {
    let selection = match resolve(site).await {
        Ok(s) => s,
        Err((e, code)) => return report(&e, code),
    };
    let mut cmd = match command(&selection, tool, args) {
        Ok(c) => c,
        Err(e) => return report(&e, 2),
    };
    let error = cmd.exec();
    report(&format!("failed to exec managed {tool}: {error}"), 74)
}

/// Report the same selection used by exec.
pub async fn run_which(tool: &str, site: Option<&str>, json: bool) -> ExitCode {
    let selection = match resolve(site).await {
        Ok(s) => s,
        Err((e, code)) => return report(&e, code),
    };
    let path = selection.installed.bin.join(tool);
    if json {
        println!(
            "{}",
            serde_json::json!({"path": path, "version": selection.installed.version, "site": selection.site, "source": selection.source})
        );
    } else {
        println!("{}", path.display());
    }
    ExitCode::SUCCESS
}

/// List installed releases from disk, without invoking Node or downloading.
pub fn list(json: bool) -> ExitCode {
    let dirs = match ActivePaths::new().resolve() {
        Ok(d) => d,
        Err(e) => return report(&e.to_string(), 74),
    };
    let config = match Config::load(&dirs.config.join("yerd.toml")) {
        Ok(c) => c,
        Err(e) => return report(&e.to_string(), 74),
    };
    let default = config.node.default.or_else(|| node::legacy_default(&dirs));
    let versions = node::installed(&dirs);
    if json {
        let values: Vec<_> = versions.iter().map(|v| serde_json::json!({"version": v.version, "default": default.as_deref() == Some(&v.version), "path": v.bin, "lts": v.lts})).collect();
        println!(
            "{}",
            serde_json::json!({"versions": values, "default": default})
        );
    } else {
        for v in versions {
            println!(
                "{}{}",
                v.version,
                if default.as_deref() == Some(&v.version) {
                    " (default)"
                } else {
                    ""
                }
            );
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn dirs(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }
    fn fake(dirs: &PlatformDirs, version: &str) -> node::InstalledNode {
        let root = node::versions_dir(dirs).join(version);
        let bin = root.join(format!("node-v{version}-test/bin"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(root.join(".version"), version).unwrap();
        // Stand in for Node: run the fake npm script, or identify this runtime.
        let body = format!("#!/bin/sh\nif [ -f \"$1\" ]; then script=$1; shift; exec /bin/sh \"$script\" \"$@\"; fi\nprintf '%s\\n' '{version}'\n");
        std::fs::write(bin.join("node"), body).unwrap();
        std::fs::set_permissions(bin.join("node"), std::fs::Permissions::from_mode(0o755)).unwrap();
        for name in ["npm", "npx"] {
            std::fs::write(
                bin.join(name),
                "#!/bin/sh\nprintf 'child='; node --version\nprintf 'arg=%s\\n' \"$1\"\n",
            )
            .unwrap();
        }
        node::resolve(dirs, version).unwrap()
    }
    #[test]
    fn precedence_nearest_file_then_site_then_global_and_no_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs(tmp.path());
        fake(&dirs, "22.9.0");
        fake(&dirs, "24.1.0");
        fake(&dirs, "24.2.0");
        let project = tmp.path().join("project");
        let nested = project.join("src/deep");
        std::fs::create_dir_all(&nested).unwrap();
        let mut cfg = Config::default();
        cfg.node.default = Some("22.9.0".to_owned());
        cfg.node
            .sites
            .insert(project.to_string_lossy().into_owned(), "24.1.0".to_owned());
        let choose = || select(&dirs, &cfg, &nested, Some(("example", &project))).unwrap();
        assert_eq!(choose().source, "site");
        // A file above the registered project cannot override its selection.
        std::fs::write(tmp.path().join(".nvmrc"), "99").unwrap();
        assert_eq!(choose().installed.version, "24.1.0");
        std::fs::write(project.join(".nvmrc"), "v22.9.0\n").unwrap();
        assert_eq!(choose().installed.version, "22.9.0");
        std::fs::write(project.join("src/.nvmrc"), "24").unwrap();
        assert_eq!(choose().installed.version, "24.2.0");
        std::fs::write(project.join("src/.nvmrc"), "99").unwrap();
        assert!(select(&dirs, &cfg, &nested, Some(("example", &project)))
            .unwrap_err()
            .contains("yerd install tool node 99"));
        std::fs::write(project.join("src/.nvmrc"), "lts/jod").unwrap();
        assert!(select(&dirs, &cfg, &nested, Some(("example", &project))).is_err());
        std::fs::remove_file(project.join("src/.nvmrc")).unwrap();
        // --site resolves using the root file rather than the caller's cwd.
        assert_eq!(
            select(&dirs, &cfg, &project, Some(("example", &project)))
                .unwrap()
                .installed
                .version,
            "22.9.0"
        );
        // A directory in place of a file is a read error, never a fallback.
        std::fs::remove_file(project.join(".nvmrc")).unwrap();
        std::fs::create_dir(project.join(".nvmrc")).unwrap();
        assert!(select(&dirs, &cfg, &nested, Some(("example", &project))).is_err());
    }
    #[test]
    fn outside_projects_uses_nearest_file_or_default() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs(tmp.path());
        fake(&dirs, "22.9.0");
        fake(&dirs, "24.1.0");
        let mut cfg = Config::default();
        cfg.node.default = Some("22.9.0".to_owned());
        let child = tmp.path().join("outside/deep");
        std::fs::create_dir_all(&child).unwrap();
        assert_eq!(select(&dirs, &cfg, &child, None).unwrap().source, "default");
        std::fs::write(tmp.path().join("outside/.nvmrc"), "v24.1").unwrap();
        assert_eq!(
            select(&dirs, &cfg, &child, None).unwrap().installed.version,
            "24.1.0"
        );
        std::fs::write(tmp.path().join("outside/.nvmrc"), "").unwrap();
        assert!(select(&dirs, &cfg, &child, None).is_err());
    }
    #[test]
    fn npm_npx_and_children_use_selected_node_and_preserve_arguments() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs(tmp.path());
        fake(&dirs, "22.9.0");
        let selected = NodeSelection {
            installed: fake(&dirs, "24.1.0"),
            site: None,
            source: "default",
        };
        for tool in ["npm", "npx"] {
            let output = command(&selected, tool, &[OsString::from("argument with spaces")])
                .unwrap()
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                "child=24.1.0\narg=argument with spaces\n"
            );
        }
        let output = command(&selected, "node", &[OsString::from("--version")])
            .unwrap()
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "24.1.0\n");
    }
}
