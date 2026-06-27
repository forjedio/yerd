//! The set of services Yerd can manage, and their pure metadata.
//!
//! Pure: no I/O. Everything here is a compile-time fact about a service (its id,
//! default port, server binary name, whether it needs datadir init, whether it
//! hosts databases). The supervisor and install layers read these facts.

use std::fmt;

/// A database / cache engine Yerd can install and supervise.
///
/// The "Redis" slot is served by **Valkey** (the BSD-licensed fork) - Redis
/// 7.4+ is SSPL/RSALv2 and not cleanly redistributable. It stays wire-compatible
/// so clients are unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Service {
    /// Redis-compatible cache/queue (Valkey under the hood).
    Redis,
    /// Oracle `MySQL`.
    MySql,
    /// `MariaDB`.
    MariaDb,
    /// `PostgreSQL`.
    Postgres,
}

/// Whether a service is a cache or a SQL database - gates the "Create Database"
/// action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    /// In-memory cache / key-value store (no SQL databases).
    Cache,
    /// SQL database server (supports `CREATE DATABASE`).
    Database,
}

impl Service {
    /// Every service, in stable order. The canonical iteration source.
    pub const ALL: [Service; 4] = [
        Service::Redis,
        Service::MySql,
        Service::MariaDb,
        Service::Postgres,
    ];

    /// The stable, lowercase wire/id string (config keys, IPC, on-disk dirs).
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Service::Redis => "redis",
            Service::MySql => "mysql",
            Service::MariaDb => "mariadb",
            Service::Postgres => "postgres",
        }
    }

    /// Parse an id string back into a [`Service`]. `None` for unknown ids.
    #[must_use]
    pub fn from_id(s: &str) -> Option<Service> {
        Service::ALL.into_iter().find(|svc| svc.id() == s)
    }

    /// Human-facing label for the GUI/CLI (carries the upstream-project note
    /// where it matters for licensing/trademark clarity).
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Service::Redis => "Redis (Valkey)",
            Service::MySql => "MySQL",
            Service::MariaDb => "MariaDB",
            Service::Postgres => "PostgreSQL",
        }
    }

    /// The IANA-default loopback port for this engine.
    #[must_use]
    pub const fn default_port(self) -> u16 {
        match self {
            Service::Redis => 6379,
            Service::MySql | Service::MariaDb => 3306,
            Service::Postgres => 5432,
        }
    }

    /// The server executable's file name inside the install's `bin/` dir.
    #[must_use]
    pub const fn server_binary(self) -> &'static str {
        match self {
            Service::Redis => "valkey-server",
            Service::MySql => "mysqld",
            Service::MariaDb => "mariadbd",
            Service::Postgres => "postgres",
        }
    }

    /// The interactive client executable in the install's `bin/` dir used to run
    /// administrative SQL (create/drop/rename/list databases). `None` for a cache
    /// (Redis has no SQL client we drive this way). The `PostgreSQL` build must
    /// include `psql` for database management to work.
    #[must_use]
    pub const fn client_binary(self) -> Option<&'static str> {
        match self {
            Service::Redis => None,
            Service::MySql => Some("mysql"),
            Service::MariaDb => Some("mariadb"),
            Service::Postgres => Some("psql"),
        }
    }

    /// The `bin/` tool that dumps a database to plain SQL on stdout, or `None`
    /// for a cache (Redis has no SQL dump). Restore reuses [`client_binary`]:
    /// the dump tool writes SQL we replay through the interactive client.
    ///
    /// [`client_binary`]: Service::client_binary
    #[must_use]
    pub const fn dump_binary(self) -> Option<&'static str> {
        match self {
            Service::Redis => None,
            Service::MySql => Some("mysqldump"),
            Service::MariaDb => Some("mariadb-dump"),
            Service::Postgres => Some("pg_dump"),
        }
    }

    /// Whether this engine requires a one-time datadir initialisation before its
    /// first start (initdb / `mysqld --initialize` / `mariadb-install-db`).
    /// Redis has none.
    #[must_use]
    pub const fn needs_init(self) -> bool {
        match self {
            Service::Redis => false,
            Service::MySql | Service::MariaDb | Service::Postgres => true,
        }
    }

    /// The `bin/` executable that performs the one-time datadir initialisation,
    /// or `None` for an engine that needs no init ([`needs_init`] is the
    /// boolean view of this). Note this is a *different* binary from the server
    /// for `MySQL` (`mysqld --initialize-insecure` is the server binary itself,
    /// but Postgres/MariaDB use a dedicated tool).
    ///
    /// [`needs_init`]: Service::needs_init
    #[must_use]
    pub const fn init_binary(self) -> Option<&'static str> {
        match self {
            Service::Redis => None,
            Service::MySql => Some("mysqld"),
            Service::MariaDb => Some("mariadb-install-db"),
            Service::Postgres => Some("initdb"),
        }
    }

    /// Whether this engine hosts SQL databases (gates "Create Database").
    #[must_use]
    pub const fn kind(self) -> ServiceKind {
        match self {
            Service::Redis => ServiceKind::Cache,
            Service::MySql | Service::MariaDb | Service::Postgres => ServiceKind::Database,
        }
    }

    /// Whether on-disk datadirs are incompatible across *major* versions (so the
    /// datadir path must be pinned per major to avoid a new server opening an
    /// old, incompatible datadir). True for Postgres; the `MySQL` family upgrades
    /// in place; Redis has no schema.
    #[must_use]
    pub const fn datadir_pinned_to_major(self) -> bool {
        matches!(self, Service::Postgres)
    }
}

impl fmt::Display for Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
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

    #[test]
    fn id_round_trips_for_every_service() {
        for svc in Service::ALL {
            assert_eq!(Service::from_id(svc.id()), Some(svc));
        }
        assert_eq!(Service::from_id("nope"), None);
    }

    #[test]
    fn all_ids_are_unique_and_lowercase() {
        let mut seen = std::collections::BTreeSet::new();
        for svc in Service::ALL {
            assert!(seen.insert(svc.id()), "duplicate id {}", svc.id());
            assert_eq!(svc.id(), svc.id().to_lowercase());
        }
        assert_eq!(seen.len(), Service::ALL.len());
    }

    #[test]
    fn default_ports_are_unprivileged() {
        for svc in Service::ALL {
            assert!(
                svc.default_port() > 1024,
                "{svc} default port is privileged"
            );
        }
    }

    #[test]
    fn redis_is_cache_and_needs_no_init() {
        assert_eq!(Service::Redis.kind(), ServiceKind::Cache);
        assert!(!Service::Redis.needs_init());
        assert_eq!(Service::Redis.server_binary(), "valkey-server");
    }

    #[test]
    fn sql_engines_are_databases_and_need_init() {
        for svc in [Service::MySql, Service::MariaDb, Service::Postgres] {
            assert_eq!(svc.kind(), ServiceKind::Database);
            assert!(svc.needs_init(), "{svc} should need init");
        }
    }

    #[test]
    fn client_binary_present_for_sql_engines_only() {
        assert_eq!(Service::Redis.client_binary(), None);
        assert_eq!(Service::MySql.client_binary(), Some("mysql"));
        assert_eq!(Service::MariaDb.client_binary(), Some("mariadb"));
        assert_eq!(Service::Postgres.client_binary(), Some("psql"));
        for svc in Service::ALL {
            assert_eq!(
                matches!(svc.kind(), ServiceKind::Database),
                svc.client_binary().is_some(),
                "{svc}"
            );
        }
    }

    #[test]
    fn dump_binary_present_for_sql_engines_only() {
        assert_eq!(Service::Redis.dump_binary(), None);
        assert_eq!(Service::MySql.dump_binary(), Some("mysqldump"));
        assert_eq!(Service::MariaDb.dump_binary(), Some("mariadb-dump"));
        assert_eq!(Service::Postgres.dump_binary(), Some("pg_dump"));
        for svc in Service::ALL {
            assert_eq!(
                matches!(svc.kind(), ServiceKind::Database),
                svc.dump_binary().is_some(),
                "{svc}"
            );
        }
    }

    #[test]
    fn init_binary_matches_needs_init() {
        assert_eq!(Service::Redis.init_binary(), None);
        assert_eq!(Service::MySql.init_binary(), Some("mysqld"));
        assert_eq!(Service::MariaDb.init_binary(), Some("mariadb-install-db"));
        assert_eq!(Service::Postgres.init_binary(), Some("initdb"));
        for svc in Service::ALL {
            assert_eq!(svc.needs_init(), svc.init_binary().is_some(), "{svc}");
        }
    }

    #[test]
    fn only_postgres_pins_datadir_to_major() {
        assert!(Service::Postgres.datadir_pinned_to_major());
        for svc in [Service::Redis, Service::MySql, Service::MariaDb] {
            assert!(!svc.datadir_pinned_to_major());
        }
    }

    #[test]
    fn display_uses_id() {
        assert_eq!(Service::Postgres.to_string(), "postgres");
        assert_eq!(Service::Redis.to_string(), "redis");
    }
}
