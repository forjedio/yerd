//! Daemon startup orchestration.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use interprocess::local_socket::tokio::Listener as IpcListener;
use interprocess::local_socket::ListenerOptions;
use tokio::sync::{Mutex, RwLock};

use yerd_core::{PhpVersion, RouterConfig, Site, SiteRouter};
use yerd_php::{
    discover_bundled, discover_mise, io::FastCgiProbe, PhpManager, SystemClock, TokioProcessSpawner,
};
use yerd_platform::{ActivePaths, ActivePortBinder, Paths, PlatformDirs, PortBinder};
use yerd_tls::{CertAuthority, Validity};

use crate::args::ServeArgs;
use crate::backend_resolver::DaemonPhpManager;
use crate::cert_store::DaemonCertStore;
use crate::error::DaemonError;
use crate::single_instance::InstanceLock;
use crate::state::DaemonState;

/// Loopback IP the embedded DNS responder binds on. The port comes from
/// [`yerd_config::Config::dns_port`] (default [`yerd_config::DEFAULT_DNS_PORT`],
/// not the mDNS-contended `5353`).
///
/// A **fixed** port (rather than ephemeral) is required so the resolver config
/// written by `yerd elevate resolver` — which hard-codes `DNS=127.0.0.1:<port>` —
/// stays valid across daemon restarts. `dns_port = 0` still means ephemeral
/// (dev/tests only); the kernel-assigned port is read back via
/// [`yerd_dns::Bound::local_addr`] and stored on [`Daemon::dns_addr`].
pub const DNS_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Everything `run()` needs to start the daemon's tasks.
pub struct Daemon {
    /// Shared runtime state: authoritative config + live router (the proxy and
    /// IPC mutation path both work through this).
    pub state: Arc<DaemonState>,
    /// Immutable TLD snapshot for the DNS responder. Taken at startup because
    /// the TLD never changes via an IPC mutation (no `SetTld`), so the DNS task
    /// must not reach into the config mutex.
    pub dns_tld: yerd_core::Tld,
    /// Where the config file was loaded from.
    pub config_path: PathBuf,
    /// Resolved per-user directories.
    pub dirs: PlatformDirs,
    /// Held until `run()` returns — releases on drop.
    pub lock: InstanceLock,
    /// PHP-FPM pool supervisor.
    pub php_manager: Arc<Mutex<DaemonPhpManager>>,
    /// TLS cert store for SNI lookups.
    pub cert_store: Arc<DaemonCertStore>,
    /// Bound HTTP listener.
    pub http_listener: tokio::net::TcpListener,
    /// Bound HTTPS listener.
    pub https_listener: tokio::net::TcpListener,
    /// Port the redirect target should advertise (≠ `https_listener` port
    /// when rootless fallback fires).
    pub https_port: u16,
    /// IPC listener (Unix socket on Unix, named pipe on Windows).
    pub ipc_listener: IpcListener,
    /// Bound DNS sockets (UDP+TCP), owned by the daemon and consumed when the
    /// DNS task is spawned.
    pub dns_bound: yerd_dns::Bound,
    /// Actual DNS bind address, read back from the kernel after the ephemeral
    /// bind. The resolver installer (post-MVP) wires `.test → this port`.
    pub dns_addr: SocketAddr,
}

/// Top-level startup: resolve platform dirs, then run the shared
/// `bring_up_with_dirs` pipeline.
pub async fn bring_up(args: &ServeArgs) -> Result<Daemon, DaemonError> {
    let dirs = ActivePaths::new().resolve()?;
    let cfg_path = args
        .config
        .clone()
        .unwrap_or_else(|| dirs.config.join("yerd.toml"));
    let config = load_or_default_config(&cfg_path)?;
    bring_up_with_dirs(dirs, config, cfg_path).await
}

/// Integration-test entry point.
///
/// Skips `ActivePaths::resolve` so the test can hand the daemon a
/// `tempfile`-rooted `PlatformDirs`. The body is identical to `bring_up`
/// from step 2 onwards.
#[doc(hidden)]
pub async fn bring_up_with_dirs(
    dirs: PlatformDirs,
    config: yerd_config::Config,
    config_path: PathBuf,
) -> Result<Daemon, DaemonError> {
    let lock = InstanceLock::acquire(&dirs)?;

    // PHP discovery — bundled first, then merge in mise (which wins on
    // collision because user-pinned versions reflect intent).
    let bundled = discover_bundled(&dirs).map_err(DaemonError::from)?;
    let mise = discover_mise().await;
    let mut binaries: BTreeMap<PhpVersion, PathBuf> = bundled.into_iter().collect();
    for (v, p) in mise {
        binaries.insert(v, p);
    }
    if binaries.is_empty() {
        tracing::warn!("no PHP versions discovered — bundled scan empty and mise unavailable");
    }

    // Load or generate the CA. Capture its path + fingerprint *before* `ca`
    // is moved into the cert store — `yerd elevate trust` needs both.
    let ca = load_or_generate_ca(&dirs)?;
    let ca_path = dirs.data.join("ca.cert.pem");
    let ca_fingerprint = yerd_platform::CaFingerprint::new(ca.fingerprint_sha256());

    let cert_store = Arc::new(DaemonCertStore::new(ca, dirs.data.join("leaves")));

    // Build the router from parked + linked sites.
    let dns_tld = config.tld.clone();
    let default_php = config.php.default;
    let router = build_router(&config, &dirs)?;
    if router.is_empty() {
        tracing::info!("no sites configured — every request will 404 until a site is added");
    }
    let router = Arc::new(RwLock::new(router));

    // Bind HTTP/HTTPS — fallback to 8080/8443 if 80/443 require elevation.
    let binder = ActivePortBinder::new();
    let pair = binder.bind_pair((config.ports.http, config.ports.https), (8080, 8443))?;
    let bound_http = pair.http.port().map_err(|source| DaemonError::Io {
        path: PathBuf::from("<http listener>"),
        source,
    })?;
    let bound_https = pair.https.port().map_err(|source| DaemonError::Io {
        path: PathBuf::from("<https listener>"),
        source,
    })?;
    if (bound_http, bound_https) != (config.ports.http, config.ports.https) {
        tracing::warn!(
            http = bound_http,
            https = bound_https,
            wanted_http = config.ports.http,
            wanted_https = config.ports.https,
            "bound rootless fallback ports; .test URLs will need explicit ports until setcap or a port-redirector is configured"
        );
    }
    let http_listener = into_tokio_listener(pair.http.listener)?;
    let tls_listener = into_tokio_listener(pair.https.listener)?;

    // PhpManager — instance_id = daemon PID disambiguates concurrent daemons
    // on the same host (different XDG_RUNTIME_DIRs notwithstanding).
    let php_manager = PhpManager::new(
        TokioProcessSpawner,
        SystemClock,
        FastCgiProbe,
        dirs.clone(),
        ActivePortBinder::new(),
        std::process::id(),
        binaries,
    );
    let php_manager = Arc::new(Mutex::new(php_manager));

    let ipc_listener = build_ipc_listener(&dirs)?;

    // Bind DNS up front (like the HTTP/HTTPS listeners) so the daemon owns the
    // sockets. Uses the fixed configured port (see `DNS_IP`) so an installed
    // resolver config keeps pointing at us across restarts.
    let dns_want = SocketAddr::new(DNS_IP, config.dns_port);
    let dns_bound = yerd_dns::Bound::bind(dns_want).await.map_err(|e| {
        tracing::error!(
            dns = %dns_want,
            "failed to bind DNS responder; another process may hold dns_port — change `dns_port` in yerd.toml or free the port"
        );
        e
    })?;
    let dns_addr = dns_bound.local_addr();
    tracing::info!(dns = %dns_addr, "DNS responder bound");

    let state = Arc::new(DaemonState {
        config: Mutex::new(config),
        router,
        dirs: dirs.clone(),
        config_path: config_path.clone(),
        default_php,
        dns_addr,
        ca_path,
        ca_fingerprint,
    });

    Ok(Daemon {
        state,
        dns_tld,
        config_path,
        dirs,
        lock,
        php_manager,
        cert_store,
        http_listener,
        https_listener: tls_listener,
        https_port: bound_https,
        ipc_listener,
        dns_bound,
        dns_addr,
    })
}

/// Build a fresh routing table from the config: scan every parked root for
/// child-directory sites, then add the explicitly linked sites (linked wins on
/// name collision). Shared by startup and the IPC mutation path so both
/// produce identical routing.
pub(crate) fn build_router(
    cfg: &yerd_config::Config,
    dirs: &PlatformDirs,
) -> Result<SiteRouter, DaemonError> {
    let sites = scan_sites(cfg, cfg.php.default, dirs)?;
    Ok(SiteRouter::from_sites(
        RouterConfig::with_tld(cfg.tld.clone()),
        sites,
    )?)
}

// ──────────────────────────────────────────────────────────────────────

fn load_or_default_config(cfg_path: &std::path::Path) -> Result<yerd_config::Config, DaemonError> {
    match yerd_config::Config::load(cfg_path) {
        Ok(c) => Ok(c),
        Err(yerd_config::ConfigError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            tracing::info!(
                config = %cfg_path.display(),
                "config file not found — using defaults for first-run boot"
            );
            Ok(yerd_config::Config::default())
        }
        Err(e) => Err(DaemonError::from(e)),
    }
}

fn load_or_generate_ca(dirs: &PlatformDirs) -> Result<CertAuthority, DaemonError> {
    let ca_pem_path = dirs.data.join("ca.cert.pem");
    let ca_key_path = dirs.data.join("ca.key.pem");
    if ca_pem_path.exists() && ca_key_path.exists() {
        let cert_pem = std::fs::read_to_string(&ca_pem_path).map_err(|source| DaemonError::Io {
            path: ca_pem_path.clone(),
            source,
        })?;
        let key_pem = std::fs::read_to_string(&ca_key_path).map_err(|source| DaemonError::Io {
            path: ca_key_path.clone(),
            source,
        })?;
        Ok(CertAuthority::from_pem(&cert_pem, &key_pem)?)
    } else {
        let validity = ca_validity()?;
        let ca = CertAuthority::generate("Yerd Local CA", validity)?;
        std::fs::create_dir_all(&dirs.data).map_err(|source| DaemonError::Io {
            path: dirs.data.clone(),
            source,
        })?;
        std::fs::write(&ca_pem_path, ca.cert_pem()).map_err(|source| DaemonError::Io {
            path: ca_pem_path.clone(),
            source,
        })?;
        std::fs::write(&ca_key_path, ca.key_pem()).map_err(|source| DaemonError::Io {
            path: ca_key_path.clone(),
            source,
        })?;
        // The CA private key is the local trust root — lock it to the owner.
        crate::secure_fs::restrict_to_owner(&ca_key_path).map_err(|source| DaemonError::Io {
            path: ca_key_path.clone(),
            source,
        })?;
        tracing::warn!(
            ca_pem = %ca_pem_path.display(),
            "generated new CA; trust-store install is deferred to a separate `yerdd install` (not in MVP)"
        );
        Ok(ca)
    }
}

fn ca_validity() -> Result<Validity, DaemonError> {
    let now = time::OffsetDateTime::now_utc();
    Ok(Validity::new(
        now - time::Duration::days(1),
        now + time::Duration::days(10 * 365),
    )?)
}

pub(crate) fn scan_sites(
    cfg: &yerd_config::Config,
    default_php: PhpVersion,
    _dirs: &PlatformDirs,
) -> Result<Vec<Site>, DaemonError> {
    let mut parked: Vec<Site> = Vec::new();
    let linked_names: std::collections::HashSet<&str> =
        cfg.linked.iter().map(yerd_core::Site::name).collect();

    for parked_root in &cfg.parked.paths {
        let entries = match std::fs::read_dir(parked_root) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(path = %parked_root, "parked path does not exist; skipping");
                continue;
            }
            Err(source) => {
                return Err(DaemonError::Io {
                    path: PathBuf::from(parked_root),
                    source,
                });
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                tracing::debug!(
                    path = %entry.path().display(),
                    "skipping non-UTF-8 directory name"
                );
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_dir() {
                continue;
            }
            let name_lower = name.to_ascii_lowercase();
            if linked_names.contains(name_lower.as_str()) {
                // Linked wins on name collision.
                continue;
            }
            match Site::parked(&name_lower, entry.path(), default_php) {
                Ok(site) => parked.push(site),
                Err(e) => {
                    tracing::debug!(
                        name = %name_lower,
                        error = %e,
                        "skipping invalid parked-site name"
                    );
                }
            }
        }
    }

    parked.extend(cfg.linked.iter().cloned());
    Ok(parked)
}

fn into_tokio_listener(
    std_listener: std::net::TcpListener,
) -> Result<tokio::net::TcpListener, DaemonError> {
    std_listener
        .set_nonblocking(true)
        .map_err(|source| DaemonError::Io {
            path: PathBuf::from("<tcp listener>"),
            source,
        })?;
    tokio::net::TcpListener::from_std(std_listener).map_err(|source| DaemonError::Io {
        path: PathBuf::from("<tcp listener>"),
        source,
    })
}

fn build_ipc_listener(dirs: &PlatformDirs) -> Result<IpcListener, DaemonError> {
    #[cfg(unix)]
    let socket_path = dirs.runtime.join("yerd.sock");
    #[cfg(unix)]
    let name = {
        use interprocess::local_socket::{GenericFilePath, ToFsName};
        let _ = std::fs::remove_file(&socket_path);
        let err_path = socket_path.clone();
        socket_path
            .clone()
            .to_fs_name::<GenericFilePath>()
            .map_err(|source| DaemonError::Io {
                path: err_path,
                source,
            })?
    };
    #[cfg(windows)]
    let name = {
        use interprocess::local_socket::{GenericNamespaced, ToNsName};
        let pipe = format!("yerd-{}", std::process::id());
        pipe.clone()
            .to_ns_name::<GenericNamespaced>()
            .map_err(|source| DaemonError::Io {
                path: PathBuf::from(&pipe),
                source,
            })?
    };
    let listener = ListenerOptions::new()
        .name(name)
        .create_tokio()
        .map_err(|source| DaemonError::Io {
            path: dirs.runtime.clone(),
            source,
        })?;
    // Lock the socket to the owning user. The runtime dir is already 0o700
    // (see `single_instance`), but tightening the socket itself is defence in
    // depth — the IPC server does no peer-credential check, so file
    // permissions are the access boundary.
    #[cfg(unix)]
    crate::secure_fs::restrict_to_owner(&socket_path).map_err(|source| DaemonError::Io {
        path: socket_path,
        source,
    })?;
    Ok(listener)
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

    fn make_dirs(tmp: &std::path::Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    #[test]
    fn scan_sites_walks_parked_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let parked_root = tmp.path().join("Sites");
        std::fs::create_dir_all(parked_root.join("app1")).unwrap();
        std::fs::create_dir_all(parked_root.join("app2")).unwrap();
        // Hidden dir is skipped.
        std::fs::create_dir_all(parked_root.join(".hidden")).unwrap();

        let mut cfg = yerd_config::Config::default();
        cfg.parked
            .paths
            .insert(parked_root.to_string_lossy().into_owned());

        let dirs = make_dirs(tmp.path());
        let sites = scan_sites(&cfg, PhpVersion::new(8, 3), &dirs).unwrap();
        let mut names: Vec<&str> = sites.iter().map(yerd_core::Site::name).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["app1", "app2"]);
    }

    #[test]
    fn scan_sites_missing_parked_root_is_warning_not_error() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = yerd_config::Config::default();
        cfg.parked.paths.insert(
            tmp.path()
                .join("does-not-exist")
                .to_string_lossy()
                .into_owned(),
        );
        let dirs = make_dirs(tmp.path());
        let sites = scan_sites(&cfg, PhpVersion::new(8, 3), &dirs).unwrap();
        assert!(sites.is_empty());
    }

    #[test]
    fn scan_sites_linked_wins_over_parked_collision() {
        let tmp = tempfile::tempdir().unwrap();
        let parked_root = tmp.path().join("Sites");
        std::fs::create_dir_all(parked_root.join("collide")).unwrap();

        let linked = Site::linked(
            "collide",
            tmp.path().join("linked-collide"),
            PhpVersion::new(8, 3),
        )
        .unwrap();
        let mut cfg = yerd_config::Config::default();
        cfg.linked.push(linked);
        cfg.parked
            .paths
            .insert(parked_root.to_string_lossy().into_owned());

        let dirs = make_dirs(tmp.path());
        let sites = scan_sites(&cfg, PhpVersion::new(8, 3), &dirs).unwrap();
        // Exactly one site, and its document_root is the linked one.
        assert_eq!(sites.len(), 1);
        assert_eq!(
            sites[0].document_root(),
            tmp.path().join("linked-collide").as_path()
        );
    }
}
