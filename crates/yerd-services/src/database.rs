//! Pure database-administration logic for the SQL engines.
//!
//! No I/O: every function here takes resolved values and returns data (a
//! validation result, a SQL string, an argv vector, a parsed list). The daemon's
//! `db_admin` glue spawns the bundled client with these and captures the output.
//!
//! This module is the **security boundary** for database administration.
//! [`validate_db_name`] is the strict policy for names Yerd creates, while
//! [`validate_existing_db_name`] accepts engine-created names used by Manage DBs.
//! SQL identifiers are always quoted and process arguments never pass through a shell.
//! Because the daemon passes each SQL string as a single `argv` element to the
//! client (never a shell), there is no shell-injection surface either.

use std::fmt;
use std::path::Path;

use crate::service::SqlEngine;

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

/// Why the name of an existing, engine-created database was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExistingDbNameError {
    /// The name was empty.
    Empty,
    /// The name contained a NUL, which cannot be represented in process arguments.
    Nul,
}

impl fmt::Display for ExistingDbNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("database name must not be empty"),
            Self::Nul => f.write_str("database name must not contain a NUL character"),
        }
    }
}

impl std::error::Error for ExistingDbNameError {}

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

/// Validate a name selected from an engine's existing databases.
///
/// Engines permit substantially more names than Yerd's portable creation policy.
/// Preserve those names exactly; only values that cannot identify a database or be
/// represented in an OS process argument are rejected.
pub fn validate_existing_db_name(name: &str) -> Result<(), ExistingDbNameError> {
    if name.is_empty() {
        return Err(ExistingDbNameError::Empty);
    }
    if name.contains('\0') {
        return Err(ExistingDbNameError::Nul);
    }
    Ok(())
}

/// Whether `name` is a built-in/system database that must not be listed,
/// dropped, or renamed. Compared case-insensitively.
#[must_use]
pub fn is_system_database(service: SqlEngine, name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let systems: &[&str] = match service {
        SqlEngine::MySql | SqlEngine::MariaDb => {
            &["information_schema", "performance_schema", "mysql", "sys"]
        }
        SqlEngine::Postgres => &["postgres", "template0", "template1"],
    };
    systems.contains(&lower.as_str())
}

/// Quote an identifier for `service`: backticks for `MySQL`/`MariaDB`, double
/// quotes for `PostgreSQL`, each doubling an embedded delimiter.
#[must_use]
pub fn quote_ident(service: SqlEngine, name: &str) -> String {
    match service {
        SqlEngine::MySql | SqlEngine::MariaDb => format!("`{}`", name.replace('`', "``")),
        SqlEngine::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// `CREATE DATABASE` statement for `name` on `service`.
#[must_use]
pub fn create_sql(service: SqlEngine, name: &str) -> String {
    format!("CREATE DATABASE {};", quote_ident(service, name))
}

/// `DROP DATABASE` statement for `name` on `service`. Postgres uses
/// `WITH (FORCE)` (PG13+) so an open session doesn't block the drop; `MySQL`/
/// `MariaDB` have no such clause.
#[must_use]
pub fn drop_sql(service: SqlEngine, name: &str) -> String {
    let ident = quote_ident(service, name);
    match service {
        SqlEngine::Postgres => format!("DROP DATABASE {ident} WITH (FORCE);"),
        SqlEngine::MySql | SqlEngine::MariaDb => format!("DROP DATABASE {ident};"),
    }
}

/// The statement that lists databases as one hexadecimal UTF-8 name per row.
/// Encoding makes the line-oriented client output reversible even when names
/// contain whitespace or line breaks.
#[must_use]
pub fn list_sql(service: SqlEngine) -> &'static str {
    match service {
        SqlEngine::MySql | SqlEngine::MariaDb => {
            "SELECT HEX(SCHEMA_NAME) FROM INFORMATION_SCHEMA.SCHEMATA;"
        }
        SqlEngine::Postgres => {
            "SELECT encode(convert_to(datname, 'UTF8'), 'hex') FROM pg_database WHERE datistemplate = false;"
        }
    }
}

/// Build the bundled-client argv to run `sql` against `service`.
///
/// `MySQL`/`MariaDB` connect over the Unix `socket` (passwordless `root@localhost`,
/// since a TCP login would fail under `skip-name-resolve`); `PostgreSQL` connects
/// over TCP loopback on `port` (its Unix socket is disabled), authenticated by
/// the `trust` line `initdb` wrote for `127.0.0.1/32`.
#[must_use]
pub fn client_args(service: SqlEngine, socket: &Path, port: u16, sql: &str) -> Vec<String> {
    match service {
        SqlEngine::MySql | SqlEngine::MariaDb => vec![
            format!("--socket={}", socket.display()),
            "--user=root".to_owned(),
            "--batch".to_owned(),
            "--skip-column-names".to_owned(),
            "-e".to_owned(),
            sql.to_owned(),
        ],
        SqlEngine::Postgres => vec![
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
    }
}

/// Wrap `db` as a libpq conninfo `dbname` value for `psql`/`pg_dump`.
///
/// `psql`/`pg_dump` treat a `--dbname` value containing `=` or a `postgresql://`
/// prefix as a full conninfo string, so a bare name like `a=b` would be misread
/// as connection keywords. Emitting the name as an explicit, single-quoted
/// `dbname='...'` conninfo (backslash-escaping `\` and `'`) forces it to be taken
/// as a literal database name for every engine-valid name.
fn pg_dbname_conninfo(db: &str) -> String {
    let escaped = db.replace('\\', "\\\\").replace('\'', "\\'");
    format!("dbname='{escaped}'")
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
pub fn dump_args(service: SqlEngine, socket: &Path, port: u16, db: &str) -> Vec<String> {
    match service {
        SqlEngine::MySql => vec![
            format!("--socket={}", socket.display()),
            "--user=root".to_owned(),
            "--routines".to_owned(),
            "--events".to_owned(),
            "--triggers".to_owned(),
            "--set-gtid-purged=OFF".to_owned(),
            "--".to_owned(),
            db.to_owned(),
        ],
        SqlEngine::MariaDb => vec![
            format!("--socket={}", socket.display()),
            "--user=root".to_owned(),
            "--routines".to_owned(),
            "--events".to_owned(),
            "--triggers".to_owned(),
            "--".to_owned(),
            db.to_owned(),
        ],
        SqlEngine::Postgres => vec![
            "--host=127.0.0.1".to_owned(),
            format!("--port={port}"),
            "--username=postgres".to_owned(),
            "--no-password".to_owned(),
            "--clean".to_owned(),
            "--if-exists".to_owned(),
            "--no-owner".to_owned(),
            "--no-privileges".to_owned(),
            format!("--dbname={}", pg_dbname_conninfo(db)),
        ],
    }
}

/// Build the restore-client argv to replay a plain-SQL stream from **stdin** into `db`.
///
/// Same connection model as [`client_args`], but targets the **requested** `db`:
/// `MySQL`/`MariaDB` take `db` positionally after a `--` end-of-options guard;
/// `PostgreSQL` connects with the db as a quoted `--dbname` conninfo (see
/// [`pg_dbname_conninfo`], not the `postgres` maintenance db) and
/// `--set=ON_ERROR_STOP=1` so a failed statement aborts with a non-zero exit instead
/// of silently partially restoring. The input file is never named here - the daemon
/// feeds it on stdin.
#[must_use]
pub fn restore_args(service: SqlEngine, socket: &Path, port: u16, db: &str) -> Vec<String> {
    match service {
        SqlEngine::MySql | SqlEngine::MariaDb => vec![
            format!("--socket={}", socket.display()),
            "--user=root".to_owned(),
            "--".to_owned(),
            db.to_owned(),
        ],
        SqlEngine::Postgres => vec![
            "--host=127.0.0.1".to_owned(),
            format!("--port={port}"),
            "--username=postgres".to_owned(),
            "--no-password".to_owned(),
            "--set=ON_ERROR_STOP=1".to_owned(),
            format!("--dbname={}", pg_dbname_conninfo(db)),
        ],
    }
}

/// Parse hexadecimal database-list stdout into exact user-visible names.
/// Invalid hexadecimal or UTF-8 indicates a broken client/query contract.
pub fn parse_db_list(service: SqlEngine, stdout: &str) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(decode_hex_name)
        .collect::<Result<_, _>>()?;
    names.retain(|name| !is_system_database(service, name));
    names.sort();
    names.dedup();
    Ok(names)
}

fn decode_hex_name(encoded: &str) -> Result<String, String> {
    if encoded.len() % 2 != 0 {
        return Err(format!(
            "database list contained odd-length hexadecimal: {encoded:?}"
        ));
    }
    let bytes = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| match pair {
            [high, low] => match (hex_nibble(*high), hex_nibble(*low)) {
                (Some(high), Some(low)) => Ok((high << 4) | low),
                _ => Err(format!(
                    "database list contained invalid hexadecimal: {encoded:?}"
                )),
            },
            _ => Err("database list decoder received an incomplete pair".to_owned()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    String::from_utf8(bytes).map_err(|_| "database list contained invalid UTF-8".to_owned())
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
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
    fn existing_name_validation_only_rejects_empty_and_nul() {
        for ok in [
            "-leading",
            "has spaces",
            "naïve 日本語",
            "quote\"",
            "back`tick",
            "a.b",
            " line\n break ",
        ] {
            assert!(
                validate_existing_db_name(ok).is_ok(),
                "{ok:?} should be valid"
            );
        }
        assert_eq!(
            validate_existing_db_name(""),
            Err(ExistingDbNameError::Empty)
        );
        assert_eq!(
            validate_existing_db_name("bad\0name"),
            Err(ExistingDbNameError::Nul)
        );
    }

    #[test]
    fn system_databases_per_engine() {
        assert!(is_system_database(SqlEngine::MySql, "mysql"));
        assert!(is_system_database(SqlEngine::MySql, "INFORMATION_SCHEMA"));
        assert!(is_system_database(SqlEngine::MariaDb, "sys"));
        assert!(!is_system_database(SqlEngine::MySql, "app"));
        assert!(is_system_database(SqlEngine::Postgres, "template0"));
        assert!(is_system_database(SqlEngine::Postgres, "postgres"));
        assert!(!is_system_database(SqlEngine::Postgres, "app"));
    }

    #[test]
    fn sql_builders_quote_per_engine() {
        assert_eq!(
            create_sql(SqlEngine::MySql, "app"),
            "CREATE DATABASE `app`;"
        );
        assert_eq!(
            create_sql(SqlEngine::Postgres, "app"),
            "CREATE DATABASE \"app\";"
        );
        assert_eq!(drop_sql(SqlEngine::MySql, "app"), "DROP DATABASE `app`;");
        assert_eq!(
            drop_sql(SqlEngine::Postgres, "app"),
            "DROP DATABASE \"app\" WITH (FORCE);"
        );
        assert_eq!(drop_sql(SqlEngine::MySql, "a`b"), "DROP DATABASE `a``b`;");
        assert_eq!(
            drop_sql(SqlEngine::Postgres, "a\"b"),
            "DROP DATABASE \"a\"\"b\" WITH (FORCE);"
        );
    }

    #[test]
    fn client_args_socket_for_mysql_tcp_for_postgres() {
        let sock = PathBuf::from("/run/yerd/mysql.sock");
        let my = client_args(SqlEngine::MySql, &sock, 3306, "SHOW DATABASES;");
        assert!(my.contains(&"--socket=/run/yerd/mysql.sock".to_owned()));
        assert!(my.contains(&"--user=root".to_owned()));
        assert!(my.iter().all(|a| !a.starts_with("--host")));
        assert_eq!(my.last().unwrap(), "SHOW DATABASES;");

        let pg = client_args(SqlEngine::Postgres, &sock, 5432, "SELECT 1;");
        assert!(pg.contains(&"--host=127.0.0.1".to_owned()));
        assert!(pg.contains(&"--port=5432".to_owned()));
        assert!(pg.contains(&"--username=postgres".to_owned()));
        assert!(pg.iter().all(|a| !a.starts_with("--socket")));
        assert_eq!(pg.last().unwrap(), "SELECT 1;");
    }

    #[test]
    fn dump_args_are_complete_and_engine_specific() {
        let sock = PathBuf::from("/run/yerd/mysql.sock");

        let my = dump_args(SqlEngine::MySql, &sock, 3306, "app");
        assert!(my.contains(&"--socket=/run/yerd/mysql.sock".to_owned()));
        assert!(my.contains(&"--user=root".to_owned()));
        for flag in ["--routines", "--events", "--triggers"] {
            assert!(my.contains(&flag.to_owned()), "mysql dump missing {flag}");
        }
        assert!(my.contains(&"--set-gtid-purged=OFF".to_owned()));
        assert!(my.iter().all(|a| a != "--databases"));
        assert_eq!(my.last().unwrap(), "app");

        let maria = dump_args(SqlEngine::MariaDb, &sock, 3306, "app");
        for flag in ["--routines", "--events", "--triggers"] {
            assert!(
                maria.contains(&flag.to_owned()),
                "mariadb dump missing {flag}"
            );
        }
        assert!(maria.iter().all(|a| !a.starts_with("--set-gtid-purged")));
        assert_eq!(maria.last().unwrap(), "app");

        let pg = dump_args(SqlEngine::Postgres, &sock, 5432, "app");
        assert!(pg.contains(&"--host=127.0.0.1".to_owned()));
        assert!(pg.contains(&"--port=5432".to_owned()));
        for flag in ["--clean", "--if-exists", "--no-owner", "--no-privileges"] {
            assert!(pg.contains(&flag.to_owned()), "pg dump missing {flag}");
        }
        assert!(pg.iter().all(|a| !a.starts_with("--socket")));
        assert_eq!(pg.last().unwrap(), "--dbname=dbname='app'");
    }

    #[test]
    fn restore_args_target_the_requested_db() {
        let sock = PathBuf::from("/run/yerd/mysql.sock");

        let my = restore_args(SqlEngine::MySql, &sock, 3306, "app");
        assert!(my.contains(&"--socket=/run/yerd/mysql.sock".to_owned()));
        assert!(my.contains(&"--user=root".to_owned()));
        assert_eq!(my[my.len() - 2], "--");
        assert_eq!(my.last().unwrap(), "app");

        let pg = restore_args(SqlEngine::Postgres, &sock, 5432, "app");
        assert!(pg.contains(&"--host=127.0.0.1".to_owned()));
        assert!(pg.contains(&"--set=ON_ERROR_STOP=1".to_owned()));
        assert!(pg.contains(&"--dbname=dbname='app'".to_owned()));
        assert!(pg.iter().all(|a| a != "--dbname=postgres"));
    }

    #[test]
    fn parse_decodes_losslessly_filters_system_and_sorts() {
        let mysql_out = "6d7973716c\n7a657461\n206c696e650a627265616b20\n617070\n";
        assert_eq!(
            parse_db_list(SqlEngine::MySql, mysql_out).unwrap(),
            vec![
                " line\nbreak ".to_owned(),
                "app".to_owned(),
                "zeta".to_owned()
            ]
        );
        let pg_out = "  706f737467726573  \n617070\ne697a5e69cace8aa9e\n6122626063\n";
        assert_eq!(
            parse_db_list(SqlEngine::Postgres, pg_out).unwrap(),
            vec!["a\"b`c".to_owned(), "app".to_owned(), "日本語".to_owned()]
        );
    }

    #[test]
    fn parse_rejects_malformed_encoded_output() {
        assert!(parse_db_list(SqlEngine::Postgres, "abc\n").is_err());
        assert!(parse_db_list(SqlEngine::MySql, "zz\n").is_err());
        assert!(parse_db_list(SqlEngine::MySql, "ff\n").is_err());
    }

    #[test]
    fn argv_preserves_unusual_names_as_one_argument() {
        let sock = PathBuf::from("/run/yerd/mysql.sock");
        let name = "- odd ` \" 日本語";
        for engine in [SqlEngine::MySql, SqlEngine::MariaDb] {
            let dump = dump_args(engine, &sock, 3306, name);
            assert_eq!(&dump[dump.len() - 2..], &["--", name]);
            let restore = restore_args(engine, &sock, 3306, name);
            assert_eq!(&restore[restore.len() - 2..], &["--", name]);
        }
        assert_eq!(
            dump_args(SqlEngine::Postgres, &sock, 5432, name).last(),
            Some(&format!("--dbname=dbname='{name}'"))
        );
        assert_eq!(
            restore_args(SqlEngine::Postgres, &sock, 5432, name).last(),
            Some(&format!("--dbname=dbname='{name}'"))
        );
    }

    #[test]
    fn pg_dbname_conninfo_forces_literal_names() {
        assert_eq!(pg_dbname_conninfo("app"), "dbname='app'");
        assert_eq!(pg_dbname_conninfo("a=b"), "dbname='a=b'");
        assert_eq!(
            pg_dbname_conninfo("postgresql://evil"),
            "dbname='postgresql://evil'"
        );
        assert_eq!(pg_dbname_conninfo("a'b"), "dbname='a\\'b'");
        assert_eq!(pg_dbname_conninfo("a\\b"), "dbname='a\\\\b'");
        assert_eq!(pg_dbname_conninfo("a\\'b"), "dbname='a\\\\\\'b'");
    }
}
