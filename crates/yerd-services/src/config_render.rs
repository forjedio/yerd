//! Pure rendering of service config files.
//!
//! No I/O - each function takes the resolved values and returns the file body as
//! a string. The manager writes it. Covers Redis/Valkey (`redis.conf`), `MySQL` and
//! `MariaDB` (`my.cnf`), and `PostgreSQL` (`postgresql.conf`), plus the include
//! line(s) that pull in the user's override sidecar files
//! ([`render_include_lines`]).

use std::path::Path;

use yerd_core::service_directives::{file_ext, OverrideDialect};

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
    let s = config_path(p);
    let escaped = s.replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn config_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Render a `MySQL` / `MariaDB` option file: loopback-only, no password.
///
/// One renderer serves both engines - `mariadbd` reads the `[mysqld]` group as
/// well as `[mariadbd]`. Key invariants:
/// - `bind-address = 127.0.0.1` + `skip-name-resolve` → reachable only from
///   localhost.
/// - The empty root password is set by `mysqld --initialize-insecure` at init
///   time, so there is no password directive here.
/// - `init-file` points at the bootstrap SQL from [`render_my_bootstrap_sql`],
///   run on every start, which makes passwordless `root` reachable over TCP
///   loopback (`--initialize-insecure` creates only the socket-matching
///   `root@localhost`, so without this a TCP client on `127.0.0.1` is rejected
///   with `[1130] Host '127.0.0.1' is not allowed`).
/// - The server runs in the foreground (no `--daemonize`); see [`crate::manager`].
/// - `pid-file` lives inside the datadir, whose parent `--initialize` creates,
///   so its directory always exists at start.
/// - `log-error` names the same instance log the manager attaches the child's
///   stderr to. Both open it in append mode, and the stderr capture is what
///   catches an option-file error raised before this directive takes effect.
#[must_use]
pub fn render_my_cnf(
    port: u16,
    datadir: &Path,
    socket: &Path,
    log_path: &Path,
    init_file: &Path,
) -> String {
    let dir = quote_conf_path(datadir);
    let sock = quote_conf_path(socket);
    let log = quote_conf_path(log_path);
    let pid = quote_conf_path(&datadir.join("mysqld.pid"));
    let init = quote_conf_path(init_file);
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
         log-error = {log}\n\
         init-file = {init}\n"
    )
}

/// Render the `MySQL`/`MariaDB` bootstrap SQL the server runs on every start (via
/// the `init-file` directive - see [`render_my_cnf`]).
///
/// It makes the passwordless `root` account reachable over **TCP loopback** so
/// apps using `DB_HOST=127.0.0.1` (Laravel's default) connect out of the box.
/// `mysqld --initialize-insecure` creates only `root@localhost`, which - under
/// `skip-name-resolve` - matches the Unix socket but not a TCP client presenting
/// the literal host `127.0.0.1`; that mismatch is the `[1130]` rejection.
///
/// Invariants that keep this safe to run on every start:
/// - Every statement is **idempotent**: `CREATE USER IF NOT EXISTS` is a no-op
///   when the account exists (so it never clobbers `MariaDB`'s own `root@127.0.0.1`,
///   which `mariadb-install-db` already creates), and `GRANT` simply re-asserts
///   the privileges.
/// - **Any statement error aborts server startup** (the `init-file` runs on
///   every normal start, not just init), so every statement must be safe to
///   re-run - hence the `IF NOT EXISTS` guards and re-assertable `GRANT`s. The
///   reader is `;`-delimited and folds the leading `--` comments into the first
///   statement (the lexer then strips them); statements are kept one-per-line
///   here for legibility, not because the reader requires it.
/// - Only the IPv4 loopback host `127.0.0.1` gets an account, matching the
///   `bind-address = 127.0.0.1` listener (the server never accepts IPv6
///   loopback, so a `root@::1` account would be dead privileged surface).
#[must_use]
pub fn render_my_bootstrap_sql() -> &'static str {
    "-- Managed by Yerd — do not edit by hand.\n\
     -- Make passwordless root reachable over TCP loopback (apps use DB_HOST=127.0.0.1).\n\
     CREATE USER IF NOT EXISTS 'root'@'127.0.0.1' IDENTIFIED BY '';\n\
     GRANT ALL PRIVILEGES ON *.* TO 'root'@'127.0.0.1' WITH GRANT OPTION;\n"
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
///
/// `preload_libraries` become a single `shared_preload_libraries` line, in the
/// given order (the caller lists `TimescaleDB` first, per upstream guidance), and
/// only when the slice is non-empty - a preload entry naming a library the
/// install does not ship makes the postmaster fail to start. The manager probes
/// the install tree and passes the resolved list; this file is regenerated in
/// full on every start (the whole config is Yerd-owned via `-c config_file=`),
/// so the line never duplicates or accumulates.
#[must_use]
pub fn render_postgresql_conf(port: u16, datadir: &Path, preload_libraries: &[&str]) -> String {
    let hba = quote_pg_string(&config_path(&datadir.join("pg_hba.conf")));
    let ident = quote_pg_string(&config_path(&datadir.join("pg_ident.conf")));
    let preload = render_preload_line(preload_libraries);
    format!(
        "# Managed by Yerd — do not edit by hand.\n\
         # Local development database (PostgreSQL).\n\
         listen_addresses = '127.0.0.1'\n\
         port = {port}\n\
         unix_socket_directories = ''\n\
         logging_collector = off\n\
         {preload}\
         hba_file = {hba}\n\
         ident_file = {ident}\n"
    )
}

/// The `shared_preload_libraries` line for the given libraries (comma-joined,
/// order preserved), or the empty string when there are none so no line is
/// emitted at all.
fn render_preload_line(libraries: &[&str]) -> String {
    if libraries.is_empty() {
        return String::new();
    }
    let value = quote_pg_string(&libraries.join(","));
    format!("shared_preload_libraries = {value}\n")
}

/// Single-quote a string for a `postgresql.conf` value, escaping embedded single
/// quotes by doubling them (the form Postgres' config parser expects).
fn quote_pg_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Render the include line(s) the manager appends to a service's Yerd-owned
/// config so the engine reads the override sidecar files in `confd_dir` after
/// Yerd's own settings.
///
/// Each dialect gets its native form:
/// - `MyCnf`: `!includedir`, which reads every `*.cnf` in the directory in name
///   order. The directive has **no quoting syntax** (unlike option *values*, cf.
///   [`quote_conf_path`]), so the path is emitted raw; a real `mysqld` parses a
///   spaced macOS state path that way, which is why this is safe.
/// - `PostgresConf`: `include_dir`, whose value is an ordinary quoted string and
///   which also reads its directory in name order.
/// - `RedisConf`: no directory form exists, so both files are named explicitly.
///   The order carries the precedence: `50-local` is read last and wins.
#[must_use]
pub fn render_include_lines(dialect: OverrideDialect, confd_dir: &Path) -> String {
    match dialect {
        OverrideDialect::MyCnf => format!("!includedir {}\n", config_path(confd_dir)),
        OverrideDialect::PostgresConf => {
            let dir = quote_pg_string(&config_path(confd_dir));
            format!("include_dir {dir}\n")
        }
        OverrideDialect::RedisConf => {
            let ext = file_ext(dialect);
            let managed = quote_conf_path(&confd_dir.join(format!("10-yerd.{ext}")));
            let local = quote_conf_path(&confd_dir.join(format!("50-local.{ext}")));
            format!("include {managed}\ninclude {local}\n")
        }
    }
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
            &PathBuf::from("/cfg/mysql-init.sql"),
        );
        assert!(conf.contains("[mysqld]"));
        assert!(conf.contains("bind-address = 127.0.0.1"));
        assert!(!conf.contains("0.0.0.0"));
        assert!(conf.contains("port = 3306"));
        assert!(conf.contains("datadir = \"/data/mysql\""));
        assert!(conf.contains("socket = \"/run/mysql.sock\""));
        assert!(conf.contains("log-error = \"/log/mysql.log\""));
        assert!(conf.contains("pid-file = \"/data/mysql/mysqld.pid\""));
        assert!(conf.contains("init-file = \"/cfg/mysql-init.sql\""));
        assert!(!conf.to_lowercase().contains("password"));
    }

    #[test]
    fn my_cnf_quotes_paths_with_spaces() {
        let conf = render_my_cnf(
            3306,
            &PathBuf::from("/Users/a b/Library/Application Support/yerd/data"),
            &PathBuf::from("/run/u/mysql.sock"),
            &PathBuf::from("/Users/a b/log.log"),
            &PathBuf::from("/Users/a b/mysql-init.sql"),
        );
        assert!(
            conf.contains("datadir = \"/Users/a b/Library/Application Support/yerd/data\""),
            "spaced datadir must be quoted intact: {conf}"
        );
        assert!(
            conf.contains("init-file = \"/Users/a b/mysql-init.sql\""),
            "spaced init-file path must be quoted intact: {conf}"
        );
    }

    #[test]
    fn my_bootstrap_sql_grants_passwordless_root_over_tcp_loopback() {
        let sql = render_my_bootstrap_sql();
        assert!(sql.contains("CREATE USER IF NOT EXISTS 'root'@'127.0.0.1' IDENTIFIED BY '';"));
        assert!(
            sql.contains("GRANT ALL PRIVILEGES ON *.* TO 'root'@'127.0.0.1' WITH GRANT OPTION;")
        );
        assert!(
            !sql.contains("::1"),
            "no IPv6-loopback account (listener is IPv4-only)"
        );
        for line in sql.lines() {
            if line.trim_start().starts_with("CREATE USER") {
                assert!(line.contains("IF NOT EXISTS"), "non-idempotent: {line}");
            }
        }
        for line in sql.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with("--") {
                continue;
            }
            assert!(
                t.ends_with(';'),
                "statement not single-line/terminated: {line}"
            );
        }
    }

    #[test]
    fn postgresql_conf_is_loopback_tcp_only() {
        let conf = render_postgresql_conf(5432, &PathBuf::from("/data/pg/data-17"), &[]);
        assert!(conf.contains("listen_addresses = '127.0.0.1'"));
        assert!(!conf.contains("0.0.0.0"));
        assert!(conf.contains("port = 5432"));
        assert!(conf.contains("unix_socket_directories = ''"));
        assert!(conf.contains("logging_collector = off"));
        assert!(conf.contains("hba_file = '/data/pg/data-17/pg_hba.conf'"));
        assert!(conf.contains("ident_file = '/data/pg/data-17/pg_ident.conf'"));
    }

    #[test]
    fn postgresql_conf_escapes_single_quotes_in_paths() {
        let conf = render_postgresql_conf(5432, &PathBuf::from("/data/o'brien/data-17"), &[]);
        assert!(
            conf.contains("hba_file = '/data/o''brien/data-17/pg_hba.conf'"),
            "single quote must be doubled: {conf}"
        );
    }

    #[test]
    fn postgresql_conf_omits_preload_line_when_no_libraries() {
        let conf = render_postgresql_conf(5432, &PathBuf::from("/data/pg/data-17"), &[]);
        assert!(
            !conf.contains("shared_preload_libraries"),
            "base install must get no preload line: {conf}"
        );
    }

    #[test]
    fn postgresql_conf_emits_preload_line_when_libraries_present() {
        let conf =
            render_postgresql_conf(5432, &PathBuf::from("/data/pg/data-17"), &["timescaledb"]);
        assert!(
            conf.contains("shared_preload_libraries = 'timescaledb'"),
            "preload line must be present and quoted: {conf}"
        );
    }

    #[test]
    fn postgresql_conf_joins_preload_libraries_in_order() {
        let conf = render_postgresql_conf(
            5432,
            &PathBuf::from("/data/pg/data-17"),
            &["timescaledb", "pg_stat_statements"],
        );
        assert!(
            conf.contains("shared_preload_libraries = 'timescaledb,pg_stat_statements'"),
            "libraries must be comma-joined in order: {conf}"
        );
    }

    #[test]
    fn include_lines_use_each_dialect_native_directive() {
        let confd = PathBuf::from("/s/services/mysql/conf.d");
        assert_eq!(
            render_include_lines(OverrideDialect::MyCnf, &confd),
            "!includedir /s/services/mysql/conf.d\n"
        );
        assert_eq!(
            render_include_lines(OverrideDialect::PostgresConf, &confd),
            "include_dir '/s/services/mysql/conf.d'\n"
        );
    }

    #[test]
    fn redis_include_lines_name_both_files_local_last() {
        let confd = PathBuf::from("/s/services/redis/conf.d");
        assert_eq!(
            render_include_lines(OverrideDialect::RedisConf, &confd),
            "include \"/s/services/redis/conf.d/10-yerd.conf\"\n\
             include \"/s/services/redis/conf.d/50-local.conf\"\n"
        );
    }

    /// The macOS state dir contains a space. `!includedir` has no quoting
    /// syntax, and a real `mysqld` reads the raw spaced path, so the `MyCnf`
    /// line must stay unquoted; the other two dialects quote as their values do.
    #[test]
    fn include_lines_carry_spaced_paths_intact() {
        let confd = PathBuf::from("/Users/a b/Library/Application Support/yerd/services/x/conf.d");
        assert_eq!(
            render_include_lines(OverrideDialect::MyCnf, &confd),
            "!includedir /Users/a b/Library/Application Support/yerd/services/x/conf.d\n"
        );
        assert_eq!(
            render_include_lines(OverrideDialect::PostgresConf, &confd),
            "include_dir '/Users/a b/Library/Application Support/yerd/services/x/conf.d'\n"
        );
        assert_eq!(
            render_include_lines(OverrideDialect::RedisConf, &confd),
            "include \"/Users/a b/Library/Application Support/yerd/services/x/conf.d/10-yerd.conf\"\n\
             include \"/Users/a b/Library/Application Support/yerd/services/x/conf.d/50-local.conf\"\n"
        );
    }

    #[test]
    fn postgres_include_line_escapes_single_quotes_in_the_path() {
        let confd = PathBuf::from("/data/o'brien/conf.d");
        assert_eq!(
            render_include_lines(OverrideDialect::PostgresConf, &confd),
            "include_dir '/data/o''brien/conf.d'\n"
        );
    }
}
