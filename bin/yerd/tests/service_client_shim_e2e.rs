//! End-to-end: the real built `yerd` binary invoked under a service client name
//! (`mysql`, `psql`, `valkey-cli`) through a symlink, against a fully faked
//! `PlatformDirs` layout with `#!/bin/sh` stubs standing in for the engine
//! binaries. Proves version selection, the connection defaults the shim adds
//! (and the user values it preserves), argument forwarding, and the failure
//! messages. Spawns the binary rather than calling
//! `service_client_shim::dispatch()` in-process, because it resolves
//! `ActivePaths::new().resolve()` internally with no dirs-injection seam.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[cfg(unix)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    use yerd_config::{Config, ServiceInstance};
    use yerd_platform::PlatformDirs;

    /// A faked home plus the dir holding the client-name symlinks to `yerd`.
    struct Fixture {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        dirs: PlatformDirs,
        links: PathBuf,
    }

    fn new_home() -> Fixture {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).expect("mkdir home");
        let dirs = PlatformDirs::for_user(&home, 0);
        let links = tmp.path().join("links");
        fs::create_dir_all(&links).expect("mkdir links");
        Fixture {
            _tmp: tmp,
            home,
            dirs,
            links,
        }
    }

    fn write_exec(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).expect("mkdir bin");
        fs::write(path, body).expect("write stub");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod +x");
    }

    fn bin_dir(h: &Fixture, id: &str, version: &str) -> PathBuf {
        h.dirs
            .data
            .join("services")
            .join(id)
            .join(version)
            .join("bin")
    }

    /// Lay down a version's server binary (what discovery keys on) and, for each
    /// of `clients`, a stub that prints the version, its args and the env the
    /// shim may set.
    fn install(h: &Fixture, id: &str, server: &str, version: &str, clients: &[&str]) {
        let bin = bin_dir(h, id, version);
        write_exec(&bin.join(server), "#!/bin/sh\nexit 0\n");
        let stub = format!(
            "#!/bin/sh\n\
             printf 'version=%s\\n' '{version}'\n\
             printf 'args=%s\\n' \"$*\"\n\
             printf 'unixport=%s\\n' \"$MYSQL_UNIX_PORT\"\n\
             printf 'pghost=%s\\n' \"$PGHOST\"\n\
             printf 'pgport=%s\\n' \"$PGPORT\"\n\
             printf 'pguser=%s\\n' \"$PGUSER\"\n"
        );
        for c in clients {
            write_exec(&bin.join(c), &stub);
        }
    }

    fn configure(h: &Fixture, id: &str, version: Option<&str>, port: Option<u16>) {
        let mut cfg = Config::default();
        cfg.services.instances.insert(
            id.to_owned(),
            ServiceInstance {
                version: version.map(str::to_owned),
                port,
                ..ServiceInstance::default()
            },
        );
        cfg.save(&h.dirs.config.join("yerd.toml"))
            .expect("save config");
    }

    fn link(h: &Fixture, name: &str) -> PathBuf {
        let path = h.links.join(name);
        if path.symlink_metadata().is_ok() {
            return path;
        }
        symlink(env!("CARGO_BIN_EXE_yerd"), &path).expect("symlink client name");
        path
    }

    fn run(h: &Fixture, name: &str, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
        let program = link(h, name);
        let mut cmd = Command::new(program);
        cmd.args(args)
            .env_clear()
            .env("HOME", &h.home)
            .env("XDG_DATA_HOME", h.home.join(".local").join("share"))
            .env("XDG_CONFIG_HOME", h.home.join(".config"))
            .env("XDG_STATE_HOME", h.home.join(".local").join("state"))
            .env("XDG_CACHE_HOME", h.home.join(".cache"));
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.output().expect("run yerd")
    }

    fn field(out: &Output, key: &str) -> String {
        assert!(
            out.status.success(),
            "shim run failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let prefix = format!("{key}=");
        stdout
            .lines()
            .find_map(|l| l.strip_prefix(prefix.as_str()))
            .unwrap_or_else(|| panic!("no {key}= line in {stdout}"))
            .to_owned()
    }

    #[test]
    fn mysql_runs_configured_version_over_the_socket_with_args_untouched() {
        let h = new_home();
        install(&h, "mysql", "mysqld", "8.0.1", &["mysql"]);
        install(&h, "mysql", "mysqld", "8.4.2", &["mysql"]);
        configure(&h, "mysql", Some("8.0.1"), Some(3307));
        let out = run(&h, "mysql", &["-u", "root", "db"], &[]);
        assert_eq!(field(&out, "version"), "8.0.1");
        assert_eq!(field(&out, "args"), "-u root db");
        let unixport = field(&out, "unixport");
        assert!(
            unixport.ends_with("services/mysql/mysql.sock"),
            "unixport {unixport}"
        );
        assert_eq!(field(&out, "pghost"), "");
    }

    #[test]
    fn mysql_falls_back_to_newest_when_configured_version_is_missing() {
        let h = new_home();
        install(&h, "mysql", "mysqld", "8.0.1", &["mysql"]);
        install(&h, "mysql", "mysqld", "8.4.2", &["mysql"]);
        configure(&h, "mysql", Some("5.7.0"), None);
        let out = run(&h, "mysql", &[], &[]);
        assert_eq!(field(&out, "version"), "8.4.2");
    }

    #[test]
    fn mysql_keeps_a_user_set_socket() {
        let h = new_home();
        install(&h, "mysql", "mysqld", "8.4.2", &["mysqldump"]);
        let out = run(
            &h,
            "mysqldump",
            &["-u", "root", "app"],
            &[("MYSQL_UNIX_PORT", "/custom.sock")],
        );
        assert_eq!(field(&out, "unixport"), "/custom.sock");
        assert_eq!(field(&out, "args"), "-u root app");
    }

    #[test]
    fn psql_exports_host_and_port_but_keeps_a_user_port() {
        let h = new_home();
        install(&h, "postgres", "postgres", "17.0", &["psql"]);
        configure(&h, "postgres", None, Some(5433));
        let out = run(&h, "psql", &["-U", "postgres"], &[]);
        assert_eq!(field(&out, "pghost"), "127.0.0.1");
        assert_eq!(field(&out, "pgport"), "5433");
        assert_eq!(field(&out, "args"), "-U postgres");
        assert_eq!(field(&out, "unixport"), "");

        let kept = run(&h, "psql", &[], &[("PGPORT", "6000")]);
        assert_eq!(field(&kept, "pghost"), "127.0.0.1");
        assert_eq!(field(&kept, "pgport"), "6000");
    }

    #[test]
    fn psql_defaults_the_user_to_postgres_unless_set_or_targeted() {
        let h = new_home();
        install(&h, "postgres", "postgres", "17.0", &["psql", "pg_dump"]);
        configure(&h, "postgres", None, Some(5433));

        let out = run(&h, "psql", &["my_app"], &[]);
        assert_eq!(field(&out, "pguser"), "postgres");
        assert_eq!(field(&out, "args"), "my_app");

        let kept = run(&h, "psql", &["my_app"], &[("PGUSER", "alice")]);
        assert_eq!(field(&kept, "pguser"), "alice");

        let remote = run(&h, "psql", &["-h", "db.example.com", "my_app"], &[]);
        assert_eq!(field(&remote, "pguser"), "");
        assert_eq!(field(&remote, "pghost"), "");

        let dump = run(&h, "pg_dump", &["my_app"], &[]);
        assert_eq!(field(&dump, "pguser"), "postgres");
    }

    #[test]
    fn valkey_cli_gets_a_port_flag_only_off_the_default_port() {
        let h = new_home();
        install(&h, "redis", "valkey-server", "8.1.0", &["valkey-cli"]);
        configure(&h, "redis", None, Some(6380));
        let out = run(&h, "valkey-cli", &["ping"], &[]);
        assert_eq!(field(&out, "args"), "-p 6380 ping");

        configure(&h, "redis", None, Some(6379));
        let default = run(&h, "valkey-cli", &["ping"], &[]);
        assert_eq!(field(&default, "args"), "ping");
    }

    #[test]
    fn fails_clearly_when_the_engine_is_not_installed() {
        let h = new_home();
        let out = run(&h, "mysql", &[], &[]);
        assert!(!out.status.success());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("is not installed"), "stderr {stderr}");
    }

    #[test]
    fn fails_clearly_when_the_build_lacks_the_client() {
        let h = new_home();
        install(&h, "mysql", "mysqld", "8.4.2", &["mysql"]);
        let out = run(&h, "mysqldump", &[], &[]);
        assert!(!out.status.success());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("is not included"), "stderr {stderr}");
    }
}
