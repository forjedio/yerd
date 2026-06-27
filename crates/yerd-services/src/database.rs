//! Pure database-administration logic for the SQL engines.
//!
//! No I/O: every function here takes resolved values and returns data (a
//! validation result, a SQL string, an argv vector, a parsed list). The daemon's
//! `db_admin` glue spawns the bundled client with these and captures the output.
//!
//! This module is the **security boundary** for "Manage DBs": [`validate_db_name`]
//! is a strict allowlist so a database name can never carry SQL/shell payload,
//! and [`quote_ident`] quotes the (already-validated) identifier belt-and-braces.
//! Because the daemon passes each SQL string as a single `argv` element to the
//! client (never a shell), there is no shell-injection surface either.

use std::fmt;
use std::path::Path;

use crate::service::Service;

/// Why a proposed database name was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbNameError {
    /// The name was empty.
    Empty,
    /// The name exceeded the 63-character limit (the lowest engine cap -
    /// `PostgreSQL` truncates at 63; `MySQL` allows 64).
    TooLong,
    /// The first character was not an ASCII letter or underscore.
    BadStart,
    /// The name contained a character outside `[A-Za-z0-9_]`.
    BadChar(char),
}

impl fmt::Display for DbNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbNameError::Empty => f.write_str("database name must not be empty"),
            DbNameError::TooLong => {
                f.write_str("database name must be 63 characters or fewer")
            }
            DbNameError::BadStart => {
                f.write_str("database name must start with a letter or underscore")
            }
            DbNameError::BadChar(c) => write!(
                f,
                "database name contains an invalid character {c:?}; use only letters, digits, and underscores"
            ),
        }
    }
}

impl std::error::Error for DbNameError {}

/// The maximum database-name length we accept (the lowest common engine cap).
const MAX_DB_NAME_LEN: usize = 63;

/// Validate a user-supplied database name against a strict allowlist:
/// non-empty, ≤ 63 chars, first char an ASCII letter or `_`, remainder ASCII
/// alphanumerics or `_`. This makes SQL injection impossible by construction -
/// no quote, backtick, semicolon, whitespace, or control character can pass.
pub fn validate_db_name(name: &str) -> Result<(), DbNameError> {
    if name.is_empty() {
        return Err(DbNameError::Empty);
    }
    if name.len() > MAX_DB_NAME_LEN {
        return Err(DbNameError::TooLong);
    }
    let mut chars = name.chars();
    if let Some(first) = chars.next() {
        if !(first.is_ascii_alphabetic() || first == '_') {
            return Err(DbNameError::BadStart);
        }
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return Err(DbNameError::BadChar(c));
        }
    }
    Ok(())
}

/// Whether `name` is a built-in/system database that must not be listed,
/// dropped, or renamed. Compared case-insensitively.
#[must_use]
pub fn is_system_database(service: Service, name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let systems: &[&str] = match service {
        Service::MySql | Service::MariaDb => {
            &["information_schema", "performance_schema", "mysql", "sys"]
        }
        Service::Postgres => &["postgres", "template0", "template1"],
        Service::Redis => &[],
    };
    systems.contains(&lower.as_str())
}

/// Quote a (validated) identifier for `service`: backticks for `MySQL`/`MariaDB`,
/// double quotes for `PostgreSQL`, each doubling the quote char. Validated names
/// contain no quote chars, so this is defence in depth.
#[must_use]
pub fn quote_ident(service: Service, name: &str) -> String {
    match service {
        Service::MySql | Service::MariaDb => format!("`{}`", name.replace('`', "``")),
        Service::Postgres | Service::Redis => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// `CREATE DATABASE` statement for `name` on `service`.
#[must_use]
pub fn create_sql(service: Service, name: &str) -> String {
    format!("CREATE DATABASE {};", quote_ident(service, name))
}

/// `DROP DATABASE` statement for `name` on `service`. Postgres uses
/// `WITH (FORCE)` (PG13+) so an open session doesn't block the drop; `MySQL`/
/// `MariaDB` have no such clause.
#[must_use]
pub fn drop_sql(service: Service, name: &str) -> String {
    let ident = quote_ident(service, name);
    match service {
        Service::Postgres => format!("DROP DATABASE {ident} WITH (FORCE);"),
        Service::MySql | Service::MariaDb | Service::Redis => format!("DROP DATABASE {ident};"),
    }
}

/// The statement that lists databases for `service` (one name per output row).
#[must_use]
pub fn list_sql(service: Service) -> &'static str {
    match service {
        Service::MySql | Service::MariaDb => "SHOW DATABASES;",
        Service::Postgres => "SELECT datname FROM pg_database WHERE datistemplate = false;",
        Service::Redis => "",
    }
}

/// Build the bundled-client argv to run `sql` against `service`.
///
/// `MySQL`/`MariaDB` connect over the Unix `socket` (passwordless `root@localhost`
/// - a TCP login would fail under `skip-name-resolve`); `PostgreSQL` connects
/// over TCP loopback on `port` (its Unix socket is disabled), authenticated by
/// the `trust` line `initdb` wrote for `127.0.0.1/32`.
#[must_use]
pub fn client_args(service: Service, socket: &Path, port: u16, sql: &str) -> Vec<String> {
    match service {
        Service::MySql | Service::MariaDb => vec![
            format!("--socket={}", socket.display()),
            "--user=root".to_owned(),
            "--batch".to_owned(),
            "--skip-column-names".to_owned(),
            "-e".to_owned(),
            sql.to_owned(),
        ],
        Service::Postgres => vec![
            "--host=127.0.0.1".to_owned(),
            format!("--port={port}"),
            "--username=postgres".to_owned(),
            "--dbname=postgres".to_owned(),
            "--no-password".to_owned(),
            "--tuples-only".to_owned(),
            "--no-align".to_owned(),
            "-c".to_owned(),
            sql.to_owned(),
        ],
        Service::Redis => Vec::new(),
    }
}

/// Build the dump-tool argv to write a complete plain-SQL dump of `db` to **stdout**.
///
/// Same connection model as [`client_args`] (`MySQL`/`MariaDB` over the Unix
/// `socket`; `PostgreSQL` over TCP loopback on `port`). The args differ per engine -
/// they are **not** uniform:
/// - `MySQL`/`MariaDB` add `--routines --events --triggers` because the dump tools
///   omit stored routines and events by default (silent data loss otherwise). The
///   single-db **positional** form (never `--databases`) emits no `CREATE DATABASE`/
///   `USE`, so the dump replays into any chosen database. `--add-drop-table` is on by
///   default, making a restore over an existing database idempotent.
/// - `MySQL` additionally passes `--set-gtid-purged=OFF` so the dump carries no
///   `SET @@GLOBAL.GTID_PURGED` (which a non-`SUPER` restore would reject).
///   `mariadb-dump` has no such flag and must not receive it.
/// - `PostgreSQL` adds `--clean --if-exists` (so a re-restore drops the dumped objects
///   first instead of erroring under `ON_ERROR_STOP`) and `--no-owner --no-privileges`
///   (so `OWNER`/`GRANT` lines can't fail when the restoring role differs).
///
/// The output file is never named here - the daemon captures stdout and writes it.
#[must_use]
pub fn dump_args(service: Service, socket: &Path, port: u16, db: &str) -> Vec<String> {
    match service {
        Service::MySql => vec![
            format!("--socket={}", socket.display()),
            "--user=root".to_owned(),
            "--routines".to_owned(),
            "--events".to_owned(),
            "--triggers".to_owned(),
            "--set-gtid-purged=OFF".to_owned(),
            db.to_owned(),
        ],
        Service::MariaDb => vec![
            format!("--socket={}", socket.display()),
            "--user=root".to_owned(),
            "--routines".to_owned(),
            "--events".to_owned(),
            "--triggers".to_owned(),
            db.to_owned(),
        ],
        Service::Postgres => vec![
            "--host=127.0.0.1".to_owned(),
            format!("--port={port}"),
            "--username=postgres".to_owned(),
            "--no-password".to_owned(),
            "--clean".to_owned(),
            "--if-exists".to_owned(),
            "--no-owner".to_owned(),
            "--no-privileges".to_owned(),
            db.to_owned(),
        ],
        Service::Redis => Vec::new(),
    }
}

/// Build the restore-client argv to replay a plain-SQL stream from **stdin** into `db`.
///
/// Same connection model as [`client_args`], but targets the **requested** `db`:
/// `MySQL`/`MariaDB` take `db` positionally; `PostgreSQL` connects with
/// `--dbname=db` (not the `postgres` maintenance db) and `--set=ON_ERROR_STOP=1` so a
/// failed statement aborts with a non-zero exit instead of silently partially
/// restoring. The input file is never named here - the daemon feeds it on stdin.
#[must_use]
pub fn restore_args(service: Service, socket: &Path, port: u16, db: &str) -> Vec<String> {
    match service {
        Service::MySql | Service::MariaDb => vec![
            format!("--socket={}", socket.display()),
            "--user=root".to_owned(),
            db.to_owned(),
        ],
        Service::Postgres => vec![
            "--host=127.0.0.1".to_owned(),
            format!("--port={port}"),
            "--username=postgres".to_owned(),
            "--no-password".to_owned(),
            "--set=ON_ERROR_STOP=1".to_owned(),
            format!("--dbname={db}"),
        ],
        Service::Redis => Vec::new(),
    }
}

/// Parse a client's database-list stdout into user-visible names: one per line,
/// trimmed, empties dropped, system databases filtered out, sorted + deduped.
#[must_use]
pub fn parse_db_list(service: Service, stdout: &str) -> Vec<String> {
    let mut names: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| !is_system_database(service, l))
        .map(str::to_owned)
        .collect();
    names.sort();
    names.dedup();
    names
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
    fn validate_accepts_reasonable_names() {
        for ok in ["app", "my_app", "App2", "_hidden", "a", "A1_b2"] {
            assert!(validate_db_name(ok).is_ok(), "{ok} should be valid");
        }
    }

    #[test]
    fn validate_rejects_injection_and_bad_input() {
        assert_eq!(validate_db_name(""), Err(DbNameError::Empty));
        assert_eq!(validate_db_name(&"a".repeat(64)), Err(DbNameError::TooLong));
        assert_eq!(validate_db_name("1db"), Err(DbNameError::BadStart));
        assert_eq!(validate_db_name("-db"), Err(DbNameError::BadStart));
        for bad in [
            "a;DROP DATABASE x",
            "a b",
            "a`b",
            "a\"b",
            "a'b",
            "a-b",
            "a.b",
            "a\\b",
            "a\nb",
            "a/b",
            "naïve",
        ] {
            assert!(
                matches!(validate_db_name(bad), Err(DbNameError::BadChar(_))),
                "{bad:?} should be BadChar"
            );
        }
    }

    #[test]
    fn system_databases_per_engine() {
        assert!(is_system_database(Service::MySql, "mysql"));
        assert!(is_system_database(Service::MySql, "INFORMATION_SCHEMA"));
        assert!(is_system_database(Service::MariaDb, "sys"));
        assert!(!is_system_database(Service::MySql, "app"));
        assert!(is_system_database(Service::Postgres, "template0"));
        assert!(is_system_database(Service::Postgres, "postgres"));
        assert!(!is_system_database(Service::Postgres, "app"));
    }

    #[test]
    fn sql_builders_quote_per_engine() {
        assert_eq!(create_sql(Service::MySql, "app"), "CREATE DATABASE `app`;");
        assert_eq!(
            create_sql(Service::Postgres, "app"),
            "CREATE DATABASE \"app\";"
        );
        assert_eq!(drop_sql(Service::MySql, "app"), "DROP DATABASE `app`;");
        assert_eq!(
            drop_sql(Service::Postgres, "app"),
            "DROP DATABASE \"app\" WITH (FORCE);"
        );
    }

    #[test]
    fn client_args_socket_for_mysql_tcp_for_postgres() {
        let sock = PathBuf::from("/run/yerd/mysql.sock");
        let my = client_args(Service::MySql, &sock, 3306, "SHOW DATABASES;");
        assert!(my.contains(&"--socket=/run/yerd/mysql.sock".to_owned()));
        assert!(my.contains(&"--user=root".to_owned()));
        assert!(my.iter().all(|a| !a.starts_with("--host")));
        assert_eq!(my.last().unwrap(), "SHOW DATABASES;");

        let pg = client_args(Service::Postgres, &sock, 5432, "SELECT 1;");
        assert!(pg.contains(&"--host=127.0.0.1".to_owned()));
        assert!(pg.contains(&"--port=5432".to_owned()));
        assert!(pg.contains(&"--username=postgres".to_owned()));
        assert!(pg.iter().all(|a| !a.starts_with("--socket")));
        assert_eq!(pg.last().unwrap(), "SELECT 1;");
    }

    #[test]
    fn dump_args_are_complete_and_engine_specific() {
        let sock = PathBuf::from("/run/yerd/mysql.sock");

        let my = dump_args(Service::MySql, &sock, 3306, "app");
        assert!(my.contains(&"--socket=/run/yerd/mysql.sock".to_owned()));
        assert!(my.contains(&"--user=root".to_owned()));
        for flag in ["--routines", "--events", "--triggers"] {
            assert!(my.contains(&flag.to_owned()), "mysql dump missing {flag}");
        }
        assert!(my.contains(&"--set-gtid-purged=OFF".to_owned()));
        assert!(my.iter().all(|a| a != "--databases"));
        assert_eq!(my.last().unwrap(), "app");

        let maria = dump_args(Service::MariaDb, &sock, 3306, "app");
        for flag in ["--routines", "--events", "--triggers"] {
            assert!(
                maria.contains(&flag.to_owned()),
                "mariadb dump missing {flag}"
            );
        }
        assert!(maria.iter().all(|a| !a.starts_with("--set-gtid-purged")));
        assert_eq!(maria.last().unwrap(), "app");

        let pg = dump_args(Service::Postgres, &sock, 5432, "app");
        assert!(pg.contains(&"--host=127.0.0.1".to_owned()));
        assert!(pg.contains(&"--port=5432".to_owned()));
        for flag in ["--clean", "--if-exists", "--no-owner", "--no-privileges"] {
            assert!(pg.contains(&flag.to_owned()), "pg dump missing {flag}");
        }
        assert!(pg.iter().all(|a| !a.starts_with("--socket")));
        assert_eq!(pg.last().unwrap(), "app");

        assert!(dump_args(Service::Redis, &sock, 6379, "app").is_empty());
    }

    #[test]
    fn restore_args_target_the_requested_db() {
        let sock = PathBuf::from("/run/yerd/mysql.sock");

        let my = restore_args(Service::MySql, &sock, 3306, "app");
        assert!(my.contains(&"--socket=/run/yerd/mysql.sock".to_owned()));
        assert!(my.contains(&"--user=root".to_owned()));
        assert_eq!(my.last().unwrap(), "app");

        let pg = restore_args(Service::Postgres, &sock, 5432, "app");
        assert!(pg.contains(&"--host=127.0.0.1".to_owned()));
        assert!(pg.contains(&"--set=ON_ERROR_STOP=1".to_owned()));
        assert!(pg.contains(&"--dbname=app".to_owned()));
        assert!(pg.iter().all(|a| a != "--dbname=postgres"));

        assert!(restore_args(Service::Redis, &sock, 6379, "app").is_empty());
    }

    #[test]
    fn parse_filters_system_and_sorts() {
        let mysql_out = "mysql\ninformation_schema\nzeta\napp\nsys\nperformance_schema\n";
        assert_eq!(
            parse_db_list(Service::MySql, mysql_out),
            vec!["app".to_owned(), "zeta".to_owned()]
        );
        let pg_out = "  postgres \n app \n template1\nbravo\n\n";
        assert_eq!(
            parse_db_list(Service::Postgres, pg_out),
            vec!["app".to_owned(), "bravo".to_owned()]
        );
    }
}
