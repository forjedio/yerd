//! Pure rendering of service config files.
//!
//! No I/O — each function takes the resolved values and returns the file body as
//! a string. The manager writes it. Phase 1 covers Redis/Valkey; `my.cnf` /
//! `postgresql.conf` land in Phase 2.

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
/// (the only metacharacters its double-quoted-string parser honours).
fn quote_conf_path(p: &Path) -> String {
    let s = p.display().to_string();
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
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
}
