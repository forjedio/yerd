//! `cargo xtask deb` — build a Linux `.deb` for the three Yerd binaries.
//!
//! Stages a Debian package tree and shells out to `dpkg-deb`. PHP is **not**
//! bundled (it is downloaded at runtime), so the package ships only `yerd`,
//! `yerdd`, `yerd-helper`, a systemd user unit, and metadata. The pure layout
//! helpers live in [`crate::pack`]; this module is the I/O glue.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::pack::{deb_filename, debian_arch, parse_version, render_control, DebMeta};

/// Maintainer recorded in the package `control` file.
const MAINTAINER: &str = "Forjed <support@forjed.io>";
/// The three binaries shipped in `usr/bin`.
const BINARIES: [&str; 3] = ["yerd", "yerdd", "yerd-helper"];

// Static package assets, embedded so they ship with the tool.
const POSTINST: &str = include_str!("../assets/postinst");
const SERVICE_UNIT: &str = include_str!("../assets/yerd.service");
const COPYRIGHT: &str = include_str!("../assets/copyright");
const CHANGELOG: &str = include_str!("../assets/changelog.Debian");

/// Arguments to `cargo xtask deb`.
#[derive(clap::Args, Debug)]
pub struct DebArgs {
    /// Target Debian architecture (defaults to the host arch).
    #[arg(long)]
    pub arch: Option<String>,
    /// Output directory for the staged tree and the `.deb` (default
    /// `target/debian`).
    #[arg(long)]
    pub out_dir: Option<PathBuf>,
    /// Skip the release build and package the existing `target/release` binaries.
    #[arg(long)]
    pub no_build: bool,
}

/// Build the `.deb` described by `args`, returning the path written.
pub fn run(args: &DebArgs) -> Result<PathBuf> {
    ensure_tool("dpkg-deb", "install it with: sudo apt install dpkg-dev")?;

    let host_arch = args
        .arch
        .clone()
        .unwrap_or_else(|| std::env::consts::ARCH.to_owned());
    let arch = debian_arch(&host_arch)
        .with_context(|| format!("unsupported architecture {host_arch:?} for .deb packaging"))?;

    let root = workspace_root();
    let release = root.join("target").join("release");

    if !args.no_build {
        build_release(&root)?;
    }
    for bin in BINARIES {
        let path = release.join(bin);
        if !path.exists() {
            bail!(
                "missing {} — run a release build first (or drop --no-build)",
                path.display()
            );
        }
    }

    let version = read_binary_version(&release.join("yerd"))?;

    let out_dir = args
        .out_dir
        .clone()
        .unwrap_or_else(|| root.join("target").join("debian"));
    std::fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    let stage = out_dir.join(format!("yerd_{version}_{arch}"));
    // Wipe any prior staging so a re-run never ships stale files.
    if let Err(e) = std::fs::remove_dir_all(&stage) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e).with_context(|| format!("clearing {}", stage.display()));
        }
    }

    stage_tree(&stage, &release, &version, arch)?;

    let deb_path = out_dir.join(deb_filename("yerd", &version, arch));
    run_checked(
        Command::new("dpkg-deb")
            .arg("--build")
            .arg("--root-owner-group")
            .arg(&stage)
            .arg(&deb_path),
        "dpkg-deb --build",
    )?;

    println!("built {}", deb_path.display());
    println!("install with:  sudo dpkg -i {}", deb_path.display());
    println!("then:          systemctl --user enable --now yerd");
    println!("persist:       loginctl enable-linger \"$USER\"");
    Ok(deb_path)
}

/// Lay down the full `DEBIAN/` + `usr/` tree under `stage`.
fn stage_tree(stage: &Path, release: &Path, version: &str, arch: &str) -> Result<()> {
    // DEBIAN/control + postinst.
    let debian = stage.join("DEBIAN");
    let meta = DebMeta {
        package: "yerd".to_owned(),
        version: version.to_owned(),
        arch: arch.to_owned(),
        maintainer: MAINTAINER.to_owned(),
        section: "devel".to_owned(),
        priority: "optional".to_owned(),
        depends: "libcap2-bin".to_owned(),
        description: "Local PHP development environment (Laravel Herd alternative)\n\
            Serves .test sites over HTTP/HTTPS with per-site PHP versions, managed\n\
            by an unprivileged per-user daemon."
            .to_owned(),
    };
    write_file(&debian.join("control"), render_control(&meta).as_bytes())?;
    let postinst = debian.join("postinst");
    write_file(&postinst, POSTINST.as_bytes())?;
    set_executable(&postinst)?;

    // usr/bin/{yerd,yerdd,yerd-helper}.
    let bin_dir = stage.join("usr").join("bin");
    std::fs::create_dir_all(&bin_dir).with_context(|| format!("creating {}", bin_dir.display()))?;
    for bin in BINARIES {
        let dst = bin_dir.join(bin);
        std::fs::copy(release.join(bin), &dst)
            .with_context(|| format!("copying {bin} into the package"))?;
        set_executable(&dst)?;
    }

    write_file(
        &stage.join("usr/lib/systemd/user/yerd.service"),
        SERVICE_UNIT.as_bytes(),
    )?;

    // usr/share/doc/yerd/{copyright, changelog.Debian.gz}.
    let doc = stage.join("usr/share/doc/yerd");
    write_file(&doc.join("copyright"), COPYRIGHT.as_bytes())?;
    write_file(
        &doc.join("changelog.Debian.gz"),
        &gzip(CHANGELOG.as_bytes())?,
    )?;
    Ok(())
}

/// gzip `data` (deterministic: the gzip header mtime defaults to 0).
fn gzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).context("gzip write")?;
    enc.finish().context("gzip finish")
}

/// Build the three release binaries.
fn build_release(root: &Path) -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(root).args([
        "build",
        "--release",
        "-p",
        "yerd",
        "-p",
        "yerdd",
        "-p",
        "yerd-helper",
    ]);
    run_checked(&mut cmd, "cargo build --release")
}

/// Read `<binary> --version` and parse the version token out of it.
fn read_binary_version(binary: &Path) -> Result<String> {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .with_context(|| format!("running {} --version", binary.display()))?;
    if !output.status.success() {
        bail!(
            "{} --version exited with {}",
            binary.display(),
            output.status
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_version(&text).with_context(|| format!("could not parse a version from {text:?}"))
}

/// Resolve the workspace root from this crate's manifest dir.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Write `contents` to `path`, creating parent directories as needed.
fn write_file(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).with_context(|| format!("chmod {}", path.display()))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Confirm a tool is on `PATH`, with a remediation hint on failure.
fn ensure_tool(tool: &str, hint: &str) -> Result<()> {
    let ok = Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if ok {
        Ok(())
    } else {
        bail!("required tool {tool:?} not found — {hint}")
    }
}

/// Run `cmd`, turning a non-zero exit or spawn failure into an error.
fn run_checked(cmd: &mut Command, label: &str) -> Result<()> {
    let status = cmd.status().with_context(|| format!("spawning {label}"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("{label} failed with {status}")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn postinst_reapplies_setcap_under_configure() {
        // The postinst must re-apply this on upgrade (dpkg wipes file caps), so
        // guard the exact line against accidental edits.
        assert!(POSTINST.contains("setcap 'cap_net_bind_service=+ep' /usr/bin/yerdd"));
        assert!(POSTINST.contains("configure)"));
    }

    #[test]
    fn service_unit_runs_yerdd_serve() {
        assert!(SERVICE_UNIT.contains("ExecStart=/usr/bin/yerdd serve"));
        assert!(SERVICE_UNIT.contains("WantedBy=default.target"));
    }
}
