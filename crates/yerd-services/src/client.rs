//! Engine client tools exposed on the user's `PATH`.
//!
//! Each engine lists a curated set of client binaries (`mysql`, `pg_dump`,
//! `valkey-cli`, ...) through [`ServiceDefinition::client_tools`]. The daemon
//! links every listed name in `{data}/bin` to the `yerd` multi-call binary,
//! which on invocation picks the version Yerd runs, adds connection defaults the
//! user has not set, and `exec`s the real binary from that version's install.
//! Resolving at call time keeps the commands working across version changes.
//!
//! Pure: no I/O. Environment lookups are injected by the caller.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use crate::service::{ServiceDefinition, ServiceRegistry};
use crate::version::ServiceVersion;

/// Valkey's default port; the shim leaves `valkey-cli` argv alone on it.
const VALKEY_DEFAULT_PORT: u16 = 6379;

/// Flags that tell `valkey-cli` where to connect, so the shim adds no `-p`.
const VALKEY_TARGET_FLAGS: [&str; 4] = ["-h", "-p", "-s", "-u"];

/// URI schemes libpq accepts as a connection string.
const POSTGRES_URI_SCHEMES: [&str; 2] = ["postgresql://", "postgres://"];

/// How a client shim points an engine's clients at Yerd's instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientDefaults {
    /// `MySQL`/`MariaDB`: export `MYSQL_UNIX_PORT` as Yerd's socket when unset.
    MySqlSocket,
    /// Postgres: export `PGHOST`, `PGPORT` and `PGUSER` (as `postgres`) when each is
    /// unset and no argument or env var names a host.
    Postgres,
    /// Valkey: prepend `-p <port>` on a non-default port when no target flag is given.
    Valkey,
}

/// The client binaries an engine exposes and how they default their connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientTools {
    /// Client executables in the install's `bin/` dir. Never server or init tools.
    pub binaries: &'static [&'static str],
    /// The connection-default rule applied to every binary in the set.
    pub defaults: ClientDefaults,
}

/// What a client shim adds before `exec`ing the real binary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectionPlan {
    /// Arguments inserted ahead of the user's own.
    pub leading_args: Vec<String>,
    /// Environment variables to set on the child.
    pub env: Vec<(&'static str, OsString)>,
}

/// The `{data}/bin` names to link to the `yerd` binary and the ones to remove.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShimPlan {
    /// Client names of engines with at least one installed version.
    pub link: Vec<&'static str>,
    /// Client names of engines with nothing installed.
    pub prune: Vec<&'static str>,
}

/// The engine whose [`ClientTools`] lists `name`, if any.
#[must_use]
pub fn owning_engine(registry: &ServiceRegistry, name: &str) -> Option<Arc<dyn ServiceDefinition>> {
    registry
        .iter()
        .find(|def| {
            def.client_tools()
                .is_some_and(|tools| tools.binaries.contains(&name))
        })
        .map(Arc::clone)
}

/// The version Yerd runs: `configured` when it parses and is installed, else the
/// newest installed version. `None` when nothing is installed.
#[must_use]
pub fn select_version(
    installed: Vec<ServiceVersion>,
    configured: Option<&str>,
) -> Option<ServiceVersion> {
    if let Some(v) = configured.and_then(|c| c.parse::<ServiceVersion>().ok()) {
        if installed.contains(&v) {
            return Some(v);
        }
    }
    installed.into_iter().max()
}

/// The connection defaults for one client invocation. A default is added only
/// when the user has not already set the matching variable, or a flag naming the
/// target. Postgres `PGUSER` is added even alongside `-U` or a connection string
/// with a user, because libpq ranks those above environment variables. Either
/// way the user's own configuration always wins.
#[must_use]
pub fn connection_plan(
    defaults: ClientDefaults,
    port: u16,
    socket: &Path,
    args: &[OsString],
    env_is_set: &dyn Fn(&str) -> bool,
) -> ConnectionPlan {
    let mut plan = ConnectionPlan::default();
    match defaults {
        ClientDefaults::MySqlSocket => {
            if !env_is_set("MYSQL_UNIX_PORT") {
                plan.env
                    .push(("MYSQL_UNIX_PORT", socket.as_os_str().to_owned()));
            }
        }
        ClientDefaults::Postgres => {
            let env_host =
                env_is_set("PGHOST") || env_is_set("PGHOSTADDR") || env_is_set("PGSERVICE");
            if env_host || args.iter().any(names_postgres_host) {
                return plan;
            }
            plan.env.push(("PGHOST", OsString::from("127.0.0.1")));
            if !env_is_set("PGPORT") {
                plan.env.push(("PGPORT", OsString::from(port.to_string())));
            }
            if !env_is_set("PGUSER") {
                plan.env.push(("PGUSER", OsString::from("postgres")));
            }
        }
        ClientDefaults::Valkey => {
            let targeted = args
                .iter()
                .any(|a| VALKEY_TARGET_FLAGS.iter().any(|f| a.as_os_str() == *f));
            if port != VALKEY_DEFAULT_PORT && !targeted {
                plan.leading_args = vec!["-p".to_owned(), port.to_string()];
            }
        }
    }
    plan
}

/// Whether a Postgres client argument names the server host: `-h`/`--host`, a
/// `host=`/`hostaddr=`/`service=` connection string (bare, or after `--dbname=`
/// or an attached `-d`), or a URI with a host or one of those query keys. The
/// shim then adds none of `PGHOST`, `PGPORT` or `PGUSER`, since libpq would
/// apply Yerd's port and user to the user's host.
fn names_postgres_host(arg: &OsString) -> bool {
    let Some(arg) = arg.to_str() else {
        return false;
    };
    if arg == "--host" || arg.starts_with("--host=") || arg.starts_with("-h") {
        return true;
    }
    let value = arg
        .strip_prefix("--dbname=")
        .or_else(|| arg.strip_prefix("-d"))
        .unwrap_or(arg);
    let conninfo_host = value.split_whitespace().any(is_target_key);
    conninfo_host || uri_names_host(value)
}

/// A conninfo key that points libpq at a server: a host, or a service file entry
/// that may name one.
fn is_target_key(kv: &str) -> bool {
    ["host=", "hostaddr=", "service="]
        .iter()
        .any(|key| kv.starts_with(key))
}

fn uri_names_host(value: &str) -> bool {
    let Some(rest) = POSTGRES_URI_SCHEMES
        .iter()
        .find_map(|scheme| value.strip_prefix(scheme))
    else {
        return false;
    };
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let has_host = !host_port.is_empty() && !host_port.starts_with(':');
    let query_host = rest
        .split_once('?')
        .is_some_and(|(_, query)| query.split('&').any(is_target_key));
    has_host || query_host
}

/// Split every engine's client names into those to link and those to prune. A
/// name is linked when its engine has an installed version and `has_client`
/// reports the binary in at least one of them, so a build that lacks a client
/// never shadows one installed another way. With nothing installed, `link` is
/// empty. `has_client(engine id, client name)` is the caller's I/O seam.
#[must_use]
pub fn shim_plan(
    registry: &ServiceRegistry,
    installed: &BTreeMap<String, Vec<ServiceVersion>>,
    has_client: &dyn Fn(&str, &str) -> bool,
) -> ShimPlan {
    let mut plan = ShimPlan::default();
    for def in registry.iter() {
        let Some(tools) = def.client_tools() else {
            continue;
        };
        let present = installed.get(def.id()).is_some_and(|v| !v.is_empty());
        for &name in tools.binaries {
            if present && has_client(def.id(), name) {
                plan.link.push(name);
            } else {
                plan.prune.push(name);
            }
        }
    }
    plan
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    type PostgresCase = (&'static [&'static str], Vec<(&'static str, OsString)>);

    fn ver(s: &str) -> ServiceVersion {
        s.parse().unwrap()
    }

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn env_of(set: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |k: &str| set.contains(&k)
    }

    fn sock() -> PathBuf {
        PathBuf::from("/run/yerd/services/mysql/mysql.sock")
    }

    #[test]
    fn mysql_socket_exports_socket_and_never_touches_argv() {
        let cases: &[&[&str]] = &[
            &[],
            &["-u", "root", "db"],
            &["-h", "127.0.0.1"],
            &["--defaults-file=x"],
        ];
        for a in cases {
            let plan = connection_plan(
                ClientDefaults::MySqlSocket,
                3306,
                &sock(),
                &args(a),
                &env_of(&[]),
            );
            assert_eq!(
                plan,
                ConnectionPlan {
                    leading_args: vec![],
                    env: vec![("MYSQL_UNIX_PORT", sock().into_os_string())],
                },
                "args {a:?}"
            );
        }
    }

    #[test]
    fn mysql_socket_keeps_user_value() {
        let plan = connection_plan(
            ClientDefaults::MySqlSocket,
            3307,
            &sock(),
            &args(&["-u", "root"]),
            &env_of(&["MYSQL_UNIX_PORT"]),
        );
        assert_eq!(plan, ConnectionPlan::default());
    }

    #[test]
    fn postgres_env_host_suppresses_all_defaults() {
        let host = ("PGHOST", OsString::from("127.0.0.1"));
        let port = ("PGPORT", OsString::from("5433"));
        let user = ("PGUSER", OsString::from("postgres"));
        let cases: &[PostgresCase] = &[
            (&[], vec![host.clone(), port.clone(), user.clone()]),
            (&["PGHOST"], vec![]),
            (&["PGHOSTADDR"], vec![]),
            (&["PGPORT"], vec![host.clone(), user.clone()]),
            (&["PGHOST", "PGPORT"], vec![]),
            (&["PGUSER"], vec![host.clone(), port.clone()]),
            (&["PGPORT", "PGUSER"], vec![host.clone()]),
            (&["PGHOST", "PGUSER"], vec![]),
            (&["PGSERVICE"], vec![]),
        ];
        for (set, want) in cases {
            let plan = connection_plan(
                ClientDefaults::Postgres,
                5433,
                &sock(),
                &args(&["-p", "9999"]),
                &env_of(set),
            );
            assert!(plan.leading_args.is_empty(), "set {set:?}");
            assert_eq!(&plan.env, want, "set {set:?}");
        }
    }

    #[test]
    fn valkey_prepends_port_only_off_default_and_untargeted() {
        let p = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<String>>();
        let cases: &[(u16, &[&str], Vec<String>)] = &[
            (6379, &[], vec![]),
            (6379, &["ping"], vec![]),
            (6380, &[], p(&["-p", "6380"])),
            (6380, &["ping"], p(&["-p", "6380"])),
            (6380, &["-h", "10.0.0.1"], vec![]),
            (6380, &["-p", "7000"], vec![]),
            (6380, &["-s", "/tmp/v.sock"], vec![]),
            (6380, &["-u", "redis://x"], vec![]),
        ];
        for (port, a, want) in cases {
            let plan = connection_plan(
                ClientDefaults::Valkey,
                *port,
                &sock(),
                &args(a),
                &env_of(&[]),
            );
            assert_eq!(&plan.leading_args, want, "port {port} args {a:?}");
            assert!(plan.env.is_empty());
        }
    }

    #[test]
    fn select_version_prefers_configured_then_newest() {
        let installed = || vec![ver("8.0.1"), ver("8.4.2"), ver("9.1.0")];
        let cases: &[(Vec<ServiceVersion>, Option<&str>, Option<&str>)] = &[
            (installed(), Some("8.4.2"), Some("8.4.2")),
            (installed(), Some("5.7.0"), Some("9.1.0")),
            (installed(), Some("not a version!"), Some("9.1.0")),
            (installed(), None, Some("9.1.0")),
            (vec![], Some("8.4.2"), None),
            (vec![], None, None),
        ];
        for (list, configured, want) in cases {
            assert_eq!(
                select_version(list.clone(), *configured),
                want.map(ver),
                "configured {configured:?}"
            );
        }
    }

    #[test]
    fn shim_plan_prunes_everything_when_nothing_installed() {
        let reg = ServiceRegistry::builtin();
        let plan = shim_plan(&reg, &BTreeMap::new(), &|_, _| true);
        assert!(plan.link.is_empty());
        for name in ["mysql", "mariadb-dump", "psql", "createdb", "valkey-cli"] {
            assert!(plan.prune.contains(&name), "{name} pruned");
        }
    }

    #[test]
    fn shim_plan_links_installed_engines_and_prunes_the_rest() {
        let reg = ServiceRegistry::builtin();
        let mut installed = BTreeMap::new();
        installed.insert("mysql".to_owned(), vec![ver("8.4.2")]);
        installed.insert("postgres".to_owned(), vec![ver("17.0")]);
        installed.insert("redis".to_owned(), vec![]);
        let plan = shim_plan(&reg, &installed, &|_, _| true);
        let mut link = plan.link.clone();
        link.sort_unstable();
        assert_eq!(
            link,
            vec![
                "createdb",
                "mysql",
                "mysqldump",
                "pg_dump",
                "pg_dumpall",
                "pg_restore",
                "psql"
            ]
        );
        let mut prune = plan.prune.clone();
        prune.sort_unstable();
        assert_eq!(prune, vec!["mariadb", "mariadb-dump", "valkey-cli"]);
    }

    #[test]
    fn shim_plan_prunes_clients_missing_from_every_installed_build() {
        let reg = ServiceRegistry::builtin();
        let mut installed = BTreeMap::new();
        installed.insert("postgres".to_owned(), vec![ver("17.0")]);
        let plan = shim_plan(&reg, &installed, &|id, name| {
            id == "postgres" && name != "createdb"
        });
        let mut link = plan.link.clone();
        link.sort_unstable();
        assert_eq!(link, vec!["pg_dump", "pg_dumpall", "pg_restore", "psql"]);
        assert!(plan.prune.contains(&"createdb"));
    }

    #[test]
    fn postgres_exports_user_even_when_an_argument_names_one() {
        let user = ("PGUSER", OsString::from("postgres"));
        let cases: &[&[&str]] = &[
            &[],
            &["my_app"],
            &["-U", "alice", "app"],
            &["-Ualice"],
            &["--username=alice"],
            &["app", "alice"],
            &["postgresql://alice@/app"],
            &["postgresql://alice@:5433/app"],
            &["user=alice dbname=app"],
        ];
        for a in cases {
            let plan = connection_plan(
                ClientDefaults::Postgres,
                5433,
                &sock(),
                &args(a),
                &env_of(&[]),
            );
            assert!(plan.leading_args.is_empty(), "args {a:?}");
            assert!(plan.env.contains(&user), "args {a:?}");
        }
    }

    #[test]
    fn postgres_adds_nothing_when_an_argument_names_a_host() {
        let cases: &[(&[&str], bool)] = &[
            (&["-h", "db.example.com"], true),
            (&["-hdb.example.com"], true),
            (&["--host", "db.example.com"], true),
            (&["--host=db.example.com"], true),
            (&["host=db.example.com dbname=app"], true),
            (&["--dbname=hostaddr=10.0.0.1 dbname=app"], true),
            (&["postgresql://user@db.example.com/app"], true),
            (&["postgres://db.example.com:5432"], true),
            (&["postgresql:///app?host=db.example.com"], true),
            (&["postgresql:///app?hostaddr=192.0.2.10"], true),
            (
                &["postgresql:///app?sslmode=disable&host=db.example.com"],
                true,
            ),
            (&["service=prod"], true),
            (&["--dbname=service=prod dbname=app"], true),
            (&["postgresql:///app?service=prod"], true),
            (&["-dpostgresql://db.example.com/app"], true),
            (&["-dhost=db.example.com dbname=app"], true),
            (&["-dmy_app"], false),
            (&["postgresql:///hostaddr=archive"], false),
            (&["postgresql:///host=archive"], false),
            (&["postgresql:///app"], false),
            (&["postgresql://user@:5433/app"], false),
            (&["-U", "postgres", "app"], false),
            (&["--help"], false),
            (&["-c", "select 'host=x'"], false),
        ];
        for (a, targeted) in cases {
            let plan = connection_plan(
                ClientDefaults::Postgres,
                5433,
                &sock(),
                &args(a),
                &env_of(&[]),
            );
            assert_eq!(plan.env.is_empty(), *targeted, "args {a:?}");
        }
    }

    #[test]
    fn owning_engine_maps_client_names() {
        let reg = ServiceRegistry::builtin();
        let cases = [
            ("mysql", Some("mysql")),
            ("mysqldump", Some("mysql")),
            ("mariadb", Some("mariadb")),
            ("mariadb-dump", Some("mariadb")),
            ("psql", Some("postgres")),
            ("pg_restore", Some("postgres")),
            ("valkey-cli", Some("redis")),
            ("mysqld", None),
            ("valkey-server", None),
            ("meilisearch", None),
            ("php", None),
        ];
        for (name, want) in cases {
            assert_eq!(
                owning_engine(&reg, name).map(|d| d.id()),
                want,
                "name {name}"
            );
        }
    }
}
