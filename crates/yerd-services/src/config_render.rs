//! Pure rendering of service config files.
//!
//! No I/O — each function takes the resolved values and returns the file body as
//! a string. The manager writes it. Covers Redis/Valkey (`redis.conf`), MySQL and
//! MariaDB (`my.cnf`), and PostgreSQL (`postgresql.conf`).

use std::path::Path;

/// Render a Redis/Valkey config: loopback-only, no password, foreground.
///
/// Key invariants:
/// - `bind 127.0.0.1` + `protected-mode yes` → reachable only from localhost.
/// - **`daemonize no`** → the process stays in the foreground as the supervised
///   master (the supervisor treats an exit of the spawned process as a crash; a
///   daemonizing server would be mis-detected as crashed and respawned, racing a
///   still-running instance).
/// - no `requirepass` → empty/no password, as specified.
#[must_use]
pub fn render_redis_conf(port: u16, datadir: &Path, logfile: &Path) -> String {
    // Paths MUST be double-quoted: the per-user data dir routinely contains a
    // space (e.g. macOS `~/Library/Application Support/io.yerd.Yerd/…`), and an
    // unquoted `dir /a b/c` would be mis-parsed by Redis/Valkey's arg splitter.
    let dir = quote_conf_path(datadir);
    let log = quote_conf_path(logfile);
    format!(
        "# Managed by Yerd — do not edit by hand.\n\
         # Local development cache (Valkey, Redis-compatible).\n\
         bind 127.0.0.1\n\
         protected-mode yes\n\
         port {port}\n\
         daemonize no\n\
         dir {dir}\n\
         logfile {log}\n\
         appendonly no\n\
         save \"\"\n"
    )
}

/// Double-quote a path for a Redis/Valkey config value, escaping `\` and `"`
/// (the only metacharacters its double-quoted-string parser honours). The same
/// double-quoted-value form is accepted by `MySQL`/`MariaDB` option files.
fn quote_conf_path(p: &Path) -> String {
    let s = p.display().to_string();
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Render a `MySQL` / `MariaDB` option file: loopback-only, no password.
///
/// One renderer serves both engines — `mariadbd` reads the `[mysqld]` group as
/// well as `[mariadbd]`. Key invariants:
/// - `bind-address = 127.0.0.1` + `skip-name-resolve` → reachable only from
///   localhost.
/// - The empty root password is set by `mysqld --initialize-insecure` at init
///   time, so there is no password directive here.
/// - The server runs in the foreground (no `--daemonize`); see [`crate::manager`].
/// - `pid-file` lives inside the datadir, whose parent `--initialize` creates,
///   so its directory always exists at start.
#[must_use]
pub fn render_my_cnf(port: u16, datadir: &Path, socket: &Path, log_path: &Path) -> String {
    let dir = quote_conf_path(datadir);
    let sock = quote_conf_path(socket);
    let log = quote_conf_path(log_path);
    let pid = quote_conf_path(&datadir.join("mysqld.pid"));
    format!(
        "# Managed by Yerd — do not edit by hand.\n\
         # Local development database (MySQL / MariaDB).\n\
         [mysqld]\n\
         bind-address = 127.0.0.1\n\
         skip-name-resolve\n\
         port = {port}\n\
         datadir = {dir}\n\
         socket = {sock}\n\
         pid-file = {pid}\n\
         log-error = {log}\n"
    )
}

/// Render a `postgresql.conf`: loopback TCP only, no Unix socket, no password.
///
/// Key invariants:
/// - `listen_addresses = '127.0.0.1'` → reachable only from localhost.
/// - **`unix_socket_directories = ''`** → no Unix socket at all; clients and the
///   readiness probe use TCP loopback. This avoids both creating a socket
///   directory and the macOS ~104-byte `sun_path` limit (the per-user state path
///   is long).
/// - `logging_collector = off` → Postgres logs to stderr, which the manager
///   redirects to the log file (so `yerd service logs postgres` works).
/// - **`hba_file` / `ident_file` are pinned to the datadir** that `initdb`
///   populated with `--auth=trust`, so passwordless loopback auth holds even
///   though this config file lives outside the datadir (`-c config_file=`).
///   `data_directory` itself comes from the `-D` command-line flag.
#[must_use]
pub fn render_postgresql_conf(port: u16, datadir: &Path) -> String {
    let hba = quote_pg_string(&datadir.join("pg_hba.conf").display().to_string());
    let ident = quote_pg_string(&datadir.join("pg_ident.conf").display().to_string());
    format!(
        "# Managed by Yerd — do not edit by hand.\n\
         # Local development database (PostgreSQL).\n\
         listen_addresses = '127.0.0.1'\n\
         port = {port}\n\
         unix_socket_directories = ''\n\
         logging_collector = off\n\
         hba_file = {hba}\n\
         ident_file = {ident}\n"
    )
}

/// Single-quote a string for a `postgresql.conf` value, escaping embedded single
/// quotes by doubling them (the form Postgres' config parser expects).
fn quote_pg_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn redis_conf_is_loopback_only_and_foreground() {
        let conf = render_redis_conf(
            6379,
            &PathBuf::from("/data/redis"),
            &PathBuf::from("/log/redis.log"),
        );
        assert!(conf.contains("bind 127.0.0.1"));
        assert!(!conf.contains("0.0.0.0"));
        assert!(conf.contains("protected-mode yes"));
        assert!(conf.contains("daemonize no"), "must run in foreground");
        assert!(!conf.contains("requirepass"), "no password");
        assert!(conf.contains("port 6379"));
        // Paths are double-quoted so a space in the data dir doesn't break parsing.
        assert!(conf.contains("dir \"/data/redis\""));
        assert!(conf.contains("logfile \"/log/redis.log\""));
    }

    #[test]
    fn redis_conf_quotes_paths_with_spaces() {
        let conf = render_redis_conf(
            6379,
            &PathBuf::from("/Users/a b/Library/Application Support/yerd"),
            &PathBuf::from("/Users/a b/log.log"),
        );
        assert!(
            conf.contains("dir \"/Users/a b/Library/Application Support/yerd\""),
            "spaced path must be quoted intact: {conf}"
        );
    }

    #[test]
    fn redis_conf_honours_custom_port() {
        let conf = render_redis_conf(6380, &PathBuf::from("/d"), &PathBuf::from("/l.log"));
        assert!(conf.contains("port 6380"));
    }

    #[test]
    fn my_cnf_is_loopback_only_no_password() {
        let conf = render_my_cnf(
            3306,
            &PathBuf::from("/data/mysql"),
            &PathBuf::from("/run/mysql.sock"),
            &PathBuf::from("/log/mysql.log"),
        );
        assert!(conf.contains("[mysqld]"));
        assert!(conf.contains("bind-address = 127.0.0.1"));
        assert!(!conf.contains("0.0.0.0"));
        assert!(conf.contains("port = 3306"));
        assert!(conf.contains("datadir = \"/data/mysql\""));
        assert!(conf.contains("socket = \"/run/mysql.sock\""));
        assert!(conf.contains("log-error = \"/log/mysql.log\""));
        // pid-file is inside the datadir (parent always exists post-init).
        assert!(conf.contains("pid-file = \"/data/mysql/mysqld.pid\""));
        // No password directive — root is left empty by --initialize-insecure.
        assert!(!conf.to_lowercase().contains("password"));
    }

    #[test]
    fn my_cnf_quotes_paths_with_spaces() {
        let conf = render_my_cnf(
            3306,
            &PathBuf::from("/Users/a b/Library/Application Support/yerd/data"),
            &PathBuf::from("/run/u/mysql.sock"),
            &PathBuf::from("/Users/a b/log.log"),
        );
        assert!(
            conf.contains("datadir = \"/Users/a b/Library/Application Support/yerd/data\""),
            "spaced datadir must be quoted intact: {conf}"
        );
    }

    #[test]
    fn postgresql_conf_is_loopback_tcp_only() {
        let conf = render_postgresql_conf(5432, &PathBuf::from("/data/pg/data-17"));
        assert!(conf.contains("listen_addresses = '127.0.0.1'"));
        assert!(!conf.contains("0.0.0.0"));
        assert!(conf.contains("port = 5432"));
        // No Unix socket; logging to stderr (manager redirects it).
        assert!(conf.contains("unix_socket_directories = ''"));
        assert!(conf.contains("logging_collector = off"));
        // hba/ident pinned to the trust-configured files initdb wrote.
        assert!(conf.contains("hba_file = '/data/pg/data-17/pg_hba.conf'"));
        assert!(conf.contains("ident_file = '/data/pg/data-17/pg_ident.conf'"));
    }

    #[test]
    fn postgresql_conf_escapes_single_quotes_in_paths() {
        // A datadir containing a single quote must double it inside the value.
        let conf = render_postgresql_conf(5432, &PathBuf::from("/data/o'brien/data-17"));
        assert!(
            conf.contains("hba_file = '/data/o''brien/data-17/pg_hba.conf'"),
            "single quote must be doubled: {conf}"
        );
    }
}
