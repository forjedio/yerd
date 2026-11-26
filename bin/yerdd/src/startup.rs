//! Daemon startup orchestration.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use interprocess::local_socket::tokio::Listener as IpcListener;
use interprocess::local_socket::ListenerOptions;
use tokio::sync::{Mutex, RwLock};

use yerd_core::{PhpVersion, Site, SiteRouter};
use yerd_php::{discover_bundled, io::FastCgiProbe, PhpManager, SystemClock, TokioProcessSpawner};
use yerd_platform::{
    ActivePaths, ActivePortBinder, ActiveTrustStore, LanIpProvider, Paths, PlatformDirs,
    PortBinder, TrustStore,
};
use yerd_tls::{CertAuthority, Validity};

use crate::args::ServeArgs;
use crate::backend_resolver::DaemonPhpManager;
use crate::cert_store::DaemonCertStore;
use crate::detect_cache::DetectCache;
use crate::error::DaemonError;
use crate::single_instance::InstanceLock;
use crate::state::DaemonState;

/// Loopback IP the embedded DNS responder binds on. The port comes from
/// [`yerd_config::Config::dns_port`] (default [`yerd_config::DEFAULT_DNS_PORT`],
/// not the mDNS-contended `5353`).
///
/// A **fixed** port (rather than ephemeral) is required so the resolver config
/// written by `yerd elevate resolver` - which hard-codes `DNS=127.0.0.1:<port>` -
/// stays valid across daemon restarts. `dns_port = 0` still means ephemeral
/// (dev/tests only); the kernel-assigned port is read back via
/// [`yerd_dns::Bound::local_addr`] and stored on [`Daemon::dns_addr`].
pub const DNS_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// The address to advertise for the DNS responder (reported by `DaemonInfo` and
/// baked into `/etc/resolver/<tld>` by `yerd elevate resolver`). In LAN mode the
/// responder binds the wildcard `0.0.0.0`, but the resolver config and the
/// host's own queries must target a concrete loopback address - so a wildcard
/// bound address is republished as `127.0.0.1` with the actual bound port.
/// Non-wildcard addresses pass through unchanged.
fn normalize_dns_advertise_addr(addr: SocketAddr) -> SocketAddr {
    match addr.ip() {
        IpAddr::V4(v4) if v4.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port())
        }
        _ => addr,
    }
}

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
    /// Held until `run()` returns - releases on drop.
    pub lock: InstanceLock,
    /// PHP-FPM pool supervisor.
    pub php_manager: Arc<Mutex<DaemonPhpManager>>,
    /// TLS cert store for SNI lookups.
    pub cert_store: Arc<DaemonCertStore>,
    /// Bound HTTP listener. `None` when the daemon could bind neither the
    /// desired nor the fallback web ports - it then runs degraded (no proxy).
    pub http_listener: Option<tokio::net::TcpListener>,
    /// Bound HTTPS listener. `None` in the same degraded case as `http_listener`.
    pub https_listener: Option<tokio::net::TcpListener>,
    /// IPC listener (Unix socket on Unix, named pipe on Windows).
    pub ipc_listener: IpcListener,
    /// Bound DNS sockets (UDP+TCP), owned by the daemon and consumed when the
    /// DNS task is spawned. `None` when the DNS port couldn't bind - the daemon
    /// then runs degraded (no name resolution) rather than aborting, mirroring
    /// the `http_listener`/`https_listener` web degrade.
    pub dns_bound: Option<yerd_dns::Bound>,
    /// Actual DNS bind address, read back from the kernel after the bind. When
    /// the DNS port couldn't bind this stays the *wanted* address (so the
    /// resolver-install probe still has a target to report against).
    pub dns_addr: SocketAddr,
    /// The addresses the DNS responder answers `.test` with. Loopback-only
    /// unless LAN mode is on, in which case it split-horizons (see
    /// [`yerd_dns::AnswerAddrs`]). Consumed when the DNS task is spawned.
    pub dns_answer: yerd_dns::AnswerAddrs,
    /// The host's discovered LAN IPv4, when LAN mode is on and discovery
    /// succeeded. `None` otherwise (LAN off, or discovery failed - fail-closed).
    pub lan_ip: Option<Ipv4Addr>,
    /// Bound mail-capture SMTP listener, when capture is enabled and the port was
    /// free. `None` = disabled, or the bind failed (non-fatal). Consumed when the
    /// mail task is spawned.
    pub mail_listener: Option<tokio::net::TcpListener>,
    /// Bound loopback tunnel origin listener (`127.0.0.1:0`) that `cloudflared`
    /// dials. `None` when the bind failed (non-fatal: sharing is unavailable).
    /// Consumed when the tunnel listener task is spawned.
    pub tunnel_listener: Option<tokio::net::TcpListener>,
    /// The `WordPress` public-URL prepend script for tunnelled requests, or
    /// `None` when writing it failed at startup.
    pub tunnel_wp_prepend: Option<PathBuf>,
    /// Parked sites renamed by public-suffix stripping whose app-side URL still
    /// needs its one-time sync (claims with `url_synced = false`), run once the
    /// autostarted services are up.
    pub renamed_sites: Vec<RenamedSite>,
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
#[allow(clippy::too_many_lines)]
pub async fn bring_up_with_dirs(
    dirs: PlatformDirs,
    mut config: yerd_config::Config,
    config_path: PathBuf,
) -> Result<Daemon, DaemonError> {
    let lock = InstanceLock::acquire(&dirs)?;

    let bundled = discover_bundled(&dirs).map_err(DaemonError::from)?;
    let binaries: BTreeMap<PhpVersion, PathBuf> = bundled.into_iter().collect();
    if binaries.is_empty() {
        tracing::warn!("no PHP versions discovered - bundled scan empty");
    }

    let ca = load_or_generate_ca(&dirs)?;
    let ca_path = dirs.data.join("ca.cert.pem");
    let ca_fingerprint = yerd_platform::CaFingerprint::new(ca.fingerprint_sha256());

    let host_roots = ActiveTrustStore
        .system_root_bundle()
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "reading host CA roots failed; PHP keeps its default trust store");
            None
        });
    let php_ca_bundle = build_php_ca_bundle(&dirs, ca.cert_pem(), host_roots.as_deref());
    let wordpress_login_prepend_script = crate::wordpress_login::write_prepend_script(
        &dirs.data,
        crate::wordpress_login::PREPEND_SCRIPT_FILE,
        crate::wordpress_login::PREPEND_SCRIPT,
    );
    let tunnel_wp_prepend = crate::wordpress_login::write_prepend_script(
        &dirs.data,
        crate::wordpress_login::PUBLIC_URL_PREPEND_SCRIPT_FILE,
        crate::wordpress_login::PUBLIC_URL_PREPEND_SCRIPT,
    );

    let cert_store = Arc::new(DaemonCertStore::new(ca, dirs.data.join("leaves")));

    let detect_cache = Arc::new(DetectCache::new());
    let dns_tld = config.tld.clone();
    let (mut router, mut wordpress_sites, mut laravel_sites, mut framework_sites) =
        build_router(&config, &dirs, &detect_cache)?;
    let renames = parked_renames(&router, &config.parked.claims);
    let migrated = rekey_parked_names(&mut config, &renames);
    if migrated {
        (router, wordpress_sites, laravel_sites, framework_sites) =
            build_router(&config, &dirs, &detect_cache)?;
    }
    let pending: HashSet<&str> = renames.iter().map(|r| r.doc_root.as_str()).collect();
    let claimed = reconcile_parked_claims(&mut config, &router, &pending);
    let stamped =
        reconcile_parked_added(&mut config, &router, crate::self_update::now_epoch(), |p| {
            Path::new(p).is_dir()
        });
    let startup_save_failed = (migrated || claimed || stamped)
        && config.save(&config_path).map_or_else(
            |e| {
                tracing::warn!(error = %e, "could not persist parked site names; the watcher retries");
                true
            },
            |()| false,
        );
    let renamed_sites = pending_url_syncs(&config);
    if router.is_empty() {
        tracing::info!("no sites configured - every request will 404 until a site is added");
    }
    let router = Arc::new(RwLock::new(router));
    let wordpress_sites = Arc::new(RwLock::new(wordpress_sites));
    let laravel_sites = Arc::new(RwLock::new(laravel_sites));
    let framework_sites = Arc::new(RwLock::new(framework_sites));
    let favourite_sites = RwLock::new(config.favourites.sites.clone());
    let site_added = RwLock::new(config.added.clone());

    let cfg_http = config.ports.http;
    let cfg_https = config.ports.https;
    let fb_http = config.ports.fallback_http;
    let fb_https = config.ports.fallback_https;
    let binder = ActivePortBinder::new();
    let (http_listener, tls_listener, bound_http, bound_https, web_unbound) = match binder
        .bind_pair(
            config.lan_enabled,
            (cfg_http, cfg_https),
            (fb_http, fb_https),
        ) {
        Ok(pair) => {
            let bound_http = pair.http.port().map_err(|source| DaemonError::Io {
                path: PathBuf::from("<http listener>"),
                source,
            })?;
            let bound_https = pair.https.port().map_err(|source| DaemonError::Io {
                path: PathBuf::from("<https listener>"),
                source,
            })?;
            if (bound_http, bound_https) != (cfg_http, cfg_https) {
                tracing::warn!(
                        http = bound_http,
                        https = bound_https,
                        wanted_http = cfg_http,
                        wanted_https = cfg_https,
                        "bound rootless fallback ports; .test URLs will need explicit ports until setcap or a port-redirector is configured"
                    );
            }
            let http_listener = into_tokio_listener(pair.http.listener)?;
            let tls_listener = into_tokio_listener(pair.https.listener)?;
            (
                Some(http_listener),
                Some(tls_listener),
                bound_http,
                bound_https,
                None,
            )
        }
        Err(e) => {
            tracing::error!(
                error = %e,
                fallback_http = fb_http,
                fallback_https = fb_https,
                "could not bind web ports; serving disabled - free the ports or change the fallback ports in Settings, then restart"
            );
            (
                None,
                None,
                0,
                0,
                Some(yerd_ipc::UnboundWeb {
                    http: fb_http,
                    https: fb_https,
                }),
            )
        }
    };

    let ffi_cache = Arc::new(crate::ffi_cache::FfiCache::new(Arc::new(
        yerd_php::TokioCommandRunner,
    )));
    let ffi_bins: Vec<(PhpVersion, PathBuf)> = binaries
        .keys()
        .map(|v| (*v, crate::php_install::cli_binary_path(&dirs, *v)))
        .collect();
    let ini_overrides = ffi_cache
        .gate_ini_overrides(
            &config.php.settings,
            &config.php.version_settings,
            &ffi_bins,
        )
        .await;
    let mut php_manager = PhpManager::new(
        TokioProcessSpawner,
        SystemClock,
        FastCgiProbe,
        dirs.clone(),
        ActivePortBinder::new(),
        std::process::id(),
        binaries,
    );
    php_manager.set_ini_settings(config.php.settings.clone());
    php_manager.set_ini_overrides(ini_overrides);
    php_manager.set_directives(config.php.directives.clone());
    php_manager.set_pool_overrides(config.php.pool.clone());
    php_manager.set_dump_ext(Some(yerd_php::DumpExtSettings {
        so_dir: dirs.data.join("php-ext"),
        ini_defines: vec![(
            "yerd_dump.state_path".to_string(),
            dirs.state
                .join("dumps")
                .join("state.json")
                .to_string_lossy()
                .into_owned(),
        )],
    }));
    php_manager.set_extensions(
        config
            .php
            .extensions
            .iter()
            .map(|(v, entries)| {
                let loads = entries
                    .iter()
                    .map(|e| yerd_php::ExtLoad {
                        path: std::path::PathBuf::from(&e.path),
                        zend: e.zend,
                    })
                    .collect();
                (*v, loads)
            })
            .collect(),
    );
    php_manager.set_xdebug(crate::xdebug::xdebug_load_map(&config, &dirs));
    crate::xdebug::ensure_output_dir(&config, &dirs);
    php_manager.set_ca_bundle(php_ca_bundle.clone());
    let php_manager = Arc::new(Mutex::new(php_manager));

    let service_manager = Arc::new(Mutex::new(crate::services::new_manager(dirs.clone())));
    let tunnel_manager = Arc::new(Mutex::new(crate::tunnel::new_manager()));

    let ipc_listener = build_ipc_listener(&dirs)?;

    // LAN mode: discover the host's routable IPv4 (fail-closed - a discovery
    // error leaves `lan_ip = None`, so DNS answers fall back to loopback while
    // the daemon still serves locally), bind the responder on `0.0.0.0` so other
    // devices can reach it, and answer `.test` with split-horizon.
    let lan_ip: Option<Ipv4Addr> = if config.lan_enabled {
        match yerd_platform::ActiveLanIpProvider::new().lan_ipv4() {
            Ok(ip) => Some(ip),
            Err(e) => {
                tracing::warn!(error = %e, "LAN IP discovery failed; .test answers fall back to loopback");
                None
            }
        }
    } else {
        None
    };
    let dns_answer = if config.lan_enabled {
        yerd_dns::AnswerAddrs::lan(lan_ip)
    } else {
        yerd_dns::AnswerAddrs::loopback()
    };

    let cfg_dns = config.dns_port;
    let dns_ip = if config.lan_enabled {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else {
        DNS_IP
    };
    let dns_want = SocketAddr::new(dns_ip, cfg_dns);
    let (dns_bound, dns_addr, dns_unbound) = match yerd_dns::Bound::bind(dns_want).await {
        Ok(bound) => {
            let addr = normalize_dns_advertise_addr(bound.local_addr());
            tracing::info!(dns = %addr, "DNS responder bound");
            (Some(bound), addr, None)
        }
        Err(e) => {
            tracing::error!(
                error = %e,
                dns = %dns_want,
                "could not bind DNS responder; name resolution disabled - free dns_port or change it in Settings, then restart"
            );
            (None, dns_want, Some(cfg_dns))
        }
    };

    let mail_enabled = config.mail.enabled;
    let mail_port = config.mail.port;
    let mail_store =
        Arc::new(
            yerd_mail::Store::open(dirs.data.join("mail")).map_err(|e| DaemonError::Io {
                path: dirs.data.join("mail"),
                source: std::io::Error::other(e.to_string()),
            })?,
        );
    let mail_listener = if mail_enabled {
        match yerd_mail::bind(mail_port).await {
            Ok(listener) => {
                tracing::info!(port = mail_port, "mail capture SMTP server bound");
                Some(listener)
            }
            Err(e) => {
                tracing::warn!(port = mail_port, error = %e, "mail capture disabled: could not bind port");
                None
            }
        }
    } else {
        None
    };
    let mail_listening = mail_listener.is_some();
    let symlink_protection_on = config.symlink_protection;

    let tunnel_listener = match tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await {
        Ok(listener) => Some(listener),
        Err(e) => {
            tracing::warn!(error = %e, "could not bind the tunnel origin listener; public sharing is unavailable");
            None
        }
    };
    let tunnel_origin_port = tunnel_listener
        .as_ref()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port());

    let state = Arc::new(DaemonState {
        config: Mutex::new(config),
        router,
        dirs: dirs.clone(),
        config_path: config_path.clone(),
        dns_addr,
        ca_path,
        ca_fingerprint,
        php_ca_bundle,
        php_updates: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        node_updates: tokio::sync::RwLock::new(std::collections::BTreeMap::new()),
        node_update_count: std::sync::atomic::AtomicU32::new(0),
        tool_updates: tokio::sync::RwLock::new(std::collections::BTreeMap::new()),
        release_notes_cache: tokio::sync::RwLock::new(None),
        contributors_cache: tokio::sync::RwLock::new(None),
        yerd_update: tokio::sync::RwLock::new(Vec::new()),
        update_snapshot: tokio::sync::RwLock::new(crate::self_update::load_snapshot(&dirs)),
        php_manager: php_manager.clone(),
        service_manager,
        tunnel_manager,
        cloudflared_resolution: tokio::sync::RwLock::new(None),
        tunnel_origin_port,
        tunnel_routes: Arc::new(RwLock::new(std::collections::HashMap::new())),
        tunnel_routes_rebuild: Mutex::new(()),
        share_warnings: Mutex::new(std::collections::HashMap::new()),
        named_stop_epoch: std::sync::atomic::AtomicU64::new(0),
        tunnel_metrics_addr: Arc::new(crate::tunnel::pick_metrics_addr),
        mail_store,
        mail: crate::state::MailRuntime {
            listening: mail_listening,
        },
        http: yerd_ipc::PortStatus {
            requested: cfg_http,
            bound: bound_http,
            fell_back: bound_http != cfg_http,
        },
        https: yerd_ipc::PortStatus {
            requested: cfg_https,
            bound: bound_https,
            fell_back: bound_https != cfg_https,
        },
        redirect_https_port: Arc::new(std::sync::atomic::AtomicU16::new(bound_https)),
        symlink_protection: Arc::new(std::sync::atomic::AtomicBool::new(symlink_protection_on)),
        web_unbound,
        dns_unbound,
        boot_id: rand_boot_id(),
        started_at: std::time::Instant::now(),
        shutdown_tx: tokio::sync::watch::channel(false).0,
        restart_requested: std::sync::atomic::AtomicBool::new(false),
        detect_cache,
        ffi_cache,
        libvips_probe: Arc::new(yerd_platform::ActiveLibvipsProbe::new()),
        network: crate::network::NetworkRuntime::production(),
        watch_dirty: tokio::sync::Notify::new(),
        config_unsaved: std::sync::atomic::AtomicBool::new(startup_save_failed),
        dumps: Arc::new(crate::dump_server::DumpStore::new()),
        shim_reconcile: tokio::sync::Mutex::new(()),
        tool_mutate: tokio::sync::Mutex::new(()),
        tunnel_mutate: tokio::sync::Mutex::new(()),
        zrok_mutate: tokio::sync::Mutex::new(()),
        php_mutate: tokio::sync::Mutex::new(()),
        php_settings_mutate: tokio::sync::Mutex::new(()),
        jobs: crate::jobs::JobRegistry::default(),
        reserved_names: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        project_file_mutate: Mutex::new(()),
        url_sync: Mutex::new(()),
        wordpress_versions: tokio::sync::RwLock::new(None),
        wordpress_login_tokens: Arc::new(crate::wordpress_login::LoginTokenRegistry::new()),
        wordpress_login_prepend_script,
        wordpress_sites,
        laravel_sites,
        framework_sites,
        favourite_sites,
        site_added,
        lan_ip,
        lan_setup_bound: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        remote_setup_code: Mutex::new(None),
        lan_setup_script_sha256: Mutex::new(None),
    });

    {
        let dumps = state.config.lock().await.dumps.clone();
        state.dumps.set_persist(dumps.persist);
        if let Err(e) = crate::dump_server::write_state_file(&state.dirs, &dumps) {
            tracing::warn!(error = %e, "failed to write initial dump state file");
        }
    }

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
        ipc_listener,
        dns_bound,
        dns_addr,
        dns_answer,
        lan_ip,
        mail_listener,
        tunnel_listener,
        tunnel_wp_prepend,
        renamed_sites,
    })
}

/// Site name → `is_wordpress`, refreshed on every router rebuild - see
/// [`build_routing`] and `DaemonState.wordpress_sites`.
pub(crate) type WordpressSites = std::collections::HashMap<String, bool>;

/// Cache of site name -> is-Laravel, built alongside [`WordpressSites`].
pub(crate) type LaravelSites = std::collections::HashMap<String, bool>;

/// Site name -> detected framework, built alongside [`WordpressSites`]. Holds
/// only sites whose framework was recognised.
pub(crate) type FrameworkSites = std::collections::HashMap<String, yerd_core::SiteFramework>;

/// Build a fresh routing table from the config: scan every parked root for
/// child-directory sites, then add the explicitly linked sites (linked wins on
/// name collision). Shared by startup and the IPC mutation path so both
/// produce identical routing. Also returns the `WordPress`-detection cache
/// (site name → `is_wordpress`) that populates `DaemonState.wordpress_sites` -
/// see [`build_routing`].
pub(crate) fn build_router(
    cfg: &yerd_config::Config,
    dirs: &PlatformDirs,
    detect_cache: &DetectCache,
) -> Result<(SiteRouter, WordpressSites, LaravelSites, FrameworkSites), DaemonError> {
    let (router, wordpress_sites, laravel_sites, framework_sites, _watch_roots) =
        build_routing(cfg, dirs, detect_cache)?;
    Ok((router, wordpress_sites, laravel_sites, framework_sites))
}

/// Like [`build_router`], but also returns the project roots the filesystem
/// watcher should keep watching: parked sites whose web root could **not** be
/// resolved yet (no framework/web-dir detected, no manual override). Resolved
/// sites are deliberately *not* watched - "don't watch what we already know".
///
/// The `WordPress`-detection map is computed here, once per rebuild, over the
/// finished router's sites - not on the `ListSites` poll path - so a marker
/// file is only ever stat'd on a mutation or a filesystem-watcher tick, no
/// matter how often the GUI polls. See `crate::wordpress_detect`. The framework
/// map is built the same way from the [`DetectCache`]'s cached signals, with the
/// `WordPress` fact taking precedence.
pub(crate) fn build_routing(
    cfg: &yerd_config::Config,
    dirs: &PlatformDirs,
    detect_cache: &DetectCache,
) -> Result<
    (
        SiteRouter,
        WordpressSites,
        LaravelSites,
        FrameworkSites,
        Vec<PathBuf>,
    ),
    DaemonError,
> {
    let (sites, watch_roots) = scan_sites(cfg, cfg.php.default, dirs, detect_cache)?;
    let router = crate::site_domains::build(cfg, sites);
    let wordpress_sites: WordpressSites = router
        .iter()
        .map(|site| {
            (
                site.name().to_owned(),
                crate::wordpress_detect::is_wordpress(&site.served_root()),
            )
        })
        .collect();
    let laravel_sites = router
        .iter()
        .map(|site| {
            (
                site.name().to_owned(),
                crate::laravel_detect::is_laravel(site.document_root()),
            )
        })
        .collect();
    let framework_sites = router
        .iter()
        .filter_map(|site| {
            let framework = if wordpress_sites.get(site.name()).copied().unwrap_or(false) {
                Some(yerd_core::SiteFramework::Wordpress)
            } else {
                yerd_core::detect_framework(&detect_cache.signals(site.document_root()))
            };
            framework.map(|f| (site.name().to_owned(), f))
        })
        .collect();
    Ok((
        router,
        wordpress_sites,
        laravel_sites,
        framework_sites,
        watch_roots,
    ))
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
                "config file not found - using defaults for first-run boot"
            );
            Ok(yerd_config::Config::default())
        }
        Err(e) => {
            tracing::error!(
                config = %cfg_path.display(),
                error = %e,
                "invalid config file - refusing to start"
            );
            Err(DaemonError::from(e))
        }
    }
}

fn load_or_generate_ca(dirs: &PlatformDirs) -> Result<CertAuthority, DaemonError> {
    let ca_pem_path = dirs.data.join("ca.cert.pem");
    let ca_key_path = dirs.data.join("ca.key.pem");
    if ca_pem_path.exists() && ca_key_path.exists() {
        crate::secure_fs::restrict_writes_to_owner(&ca_pem_path).map_err(|source| {
            DaemonError::Io {
                path: ca_pem_path.clone(),
                source,
            }
        })?;
        crate::secure_fs::restrict_to_owner(&ca_key_path).map_err(|source| DaemonError::Io {
            path: ca_key_path.clone(),
            source,
        })?;
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
        let ca = CertAuthority::generate(yerd_core::CA_COMMON_NAME, validity)?;
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
        crate::secure_fs::restrict_to_owner(&ca_key_path).map_err(|source| DaemonError::Io {
            path: ca_key_path.clone(),
            source,
        })?;
        crate::secure_fs::restrict_writes_to_owner(&ca_pem_path).map_err(|source| {
            DaemonError::Io {
                path: ca_pem_path.clone(),
                source,
            }
        })?;
        tracing::warn!(
            ca_pem = %ca_pem_path.display(),
            "generated new CA; trust-store install is deferred to a separate `yerdd install` (not in MVP)"
        );
        Ok(ca)
    }
}

/// Build `{data}/cacert.pem` = host public roots + the Yerd CA, the bundle the
/// bundled PHP verifies TLS against so it trusts `.test` HTTPS.
///
/// Returns `Some(path)` when `roots` contains public roots (at least one
/// `CERTIFICATE` block): the bundle is rewritten and its path returned. When no
/// usable roots are present the file is **not written or modified**; instead a
/// previously-written bundle that still embeds the current CA plus public roots
/// is reused (`Some(path)`), so a transient host-roots read failure doesn't
/// disable CA wiring while a good bundle sits on disk. Only when neither fresh
/// roots nor a reusable bundle exist is `None` returned, leaving PHP's
/// compiled-in default trust store untouched rather than pointing it at a
/// rootless bundle (which would break public-internet HTTPS). Best-effort: a
/// write failure logs and yields `None`. `roots` is passed in (not read here) so
/// the branches are unit-testable without touching the host trust store.
pub(crate) fn build_php_ca_bundle(
    dirs: &PlatformDirs,
    ca_cert_pem: &str,
    roots: Option<&str>,
) -> Option<PathBuf> {
    let path = dirs.data.join("cacert.pem");
    let Some(roots_pem) = roots.filter(|r| r.contains("-----BEGIN CERTIFICATE-----")) else {
        return reuse_existing_php_ca_bundle(&path, ca_cert_pem);
    };
    let bundle = yerd_tls::compose_ca_bundle(roots_pem, ca_cert_pem);

    if let Err(e) = std::fs::create_dir_all(&dirs.data) {
        tracing::warn!(error = %e, "could not create data dir for PHP CA bundle");
        return None;
    }
    if let Err(e) = yerd_php::io::atomic_write::write(&path, bundle.as_bytes()) {
        tracing::warn!(error = %e, path = %path.display(), "could not write PHP CA bundle");
        return None;
    }
    if let Err(e) = crate::secure_fs::restrict_writes_to_owner(&path) {
        tracing::warn!(error = %e, path = %path.display(), "could not set PHP CA bundle permissions");
    }
    tracing::info!(path = %path.display(), "wrote PHP CA bundle (host roots + Yerd CA)");
    Some(path)
}

/// Keep PHP CA wiring alive when fresh host roots are unavailable: if a bundle
/// previously written to `path` still embeds the current CA plus public roots
/// (at least two certificate blocks), reuse it by returning `Some(path)`. Never
/// returns a rootless bundle, and never writes or modifies the file; returns
/// `None` (leaving PHP's default trust store) when no reusable bundle exists.
fn reuse_existing_php_ca_bundle(path: &std::path::Path, ca_cert_pem: &str) -> Option<PathBuf> {
    let ca = ca_cert_pem.trim();
    let existing = std::fs::read_to_string(path).ok()?;
    let cert_blocks = existing.matches("-----BEGIN CERTIFICATE-----").count();
    if !ca.is_empty() && existing.contains(ca) && cert_blocks >= 2 {
        tracing::warn!(
            path = %path.display(),
            "host CA roots unavailable; reusing existing PHP CA bundle (already has public roots + Yerd CA)"
        );
        return Some(path.to_path_buf());
    }
    tracing::warn!(
        "no host CA roots and no reusable bundle; leaving PHP's default trust store in place to avoid breaking public HTTPS"
    );
    None
}

fn ca_validity() -> Result<Validity, DaemonError> {
    let now = time::OffsetDateTime::now_utc();
    Ok(Validity::new(
        now - time::Duration::days(1),
        now + time::Duration::days(10 * 365),
    )?)
}

/// Discover every site: each parked root's child directories plus the linked
/// sites. Parked candidates are gathered root-major (roots in their stored,
/// lexicographic order, folders sorted within each root) and named by
/// [`assign_parked_names`], so a folder ending in a public suffix
/// (`laravel.com`) is served under its stripped name (`laravel`) when that is
/// free and uncontested, and under its full slug (`laravel-com`) otherwise.
pub(crate) fn scan_sites(
    cfg: &yerd_config::Config,
    default_php: PhpVersion,
    _dirs: &PlatformDirs,
    detect_cache: &DetectCache,
) -> Result<(Vec<Site>, Vec<PathBuf>), DaemonError> {
    let mut parked: Vec<Site> = Vec::new();
    let mut watch_roots: Vec<PathBuf> = Vec::new();

    let mut found_roots: Vec<(String, PathBuf)> = Vec::new();
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
        let mut found: Vec<(String, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| parked_candidate(&entry).map(|folder| (folder, entry.path())))
            .collect();
        found.sort();
        found_roots.extend(found);
    }

    let doc_roots: Vec<String> = found_roots
        .iter()
        .map(|(_, path)| path.to_string_lossy().into_owned())
        .collect();
    let candidates: Vec<ParkedCandidate<'_>> = found_roots
        .iter()
        .zip(&doc_roots)
        .map(|((folder, _), doc_root)| ParkedCandidate { folder, doc_root })
        .collect();
    let names = assign_parked_names(
        &candidates,
        &TakenNames::from_config(cfg),
        &cfg.parked.claims,
    );
    for ((folder, doc_root), name) in found_roots.iter().zip(names) {
        let Some(site_name) = name else {
            tracing::debug!(name = %folder, "skipping parked dir with no valid or free site name");
            continue;
        };
        if let Some(site) = build_parked_site(
            &site_name,
            doc_root,
            default_php,
            cfg,
            detect_cache,
            &mut watch_roots,
        ) {
            parked.push(site);
        }
    }

    parked.extend(cfg.linked.iter().cloned());
    Ok((parked, watch_roots))
}

/// The folder name of one parked-directory entry, or `None` to skip it
/// (non-UTF-8, hidden, or not a directory).
fn parked_candidate(entry: &std::fs::DirEntry) -> Option<String> {
    let file_name = entry.file_name();
    let Some(name) = file_name.to_str() else {
        tracing::debug!(
            path = %entry.path().display(),
            "skipping non-UTF-8 directory name"
        );
        return None;
    };
    if name.starts_with('.') {
        return None;
    }
    if !entry.metadata().ok()?.is_dir() {
        return None;
    }
    Some(name.to_owned())
}

/// One parked folder awaiting a name.
struct ParkedCandidate<'a> {
    folder: &'a str,
    doc_root: &'a str,
}

/// Names held by something other than a parked folder, which a folder's
/// suffix-stripped name must not take.
struct TakenNames<'a> {
    linked: HashSet<&'a str>,
    proxies: HashSet<&'a str>,
    /// Exact (non-wildcard) domains explicitly added in `[domains]`, each with
    /// the parked document root that added it (`None` for a linked site or a
    /// proxy). An explicit domain beats a site's implicit apex when claims are
    /// built, so a stripped name equal to one would leave the site unreachable.
    added: Vec<(Option<&'a str>, &'a str)>,
}

impl<'a> TakenNames<'a> {
    fn from_config(cfg: &'a yerd_config::Config) -> Self {
        let unowned = cfg
            .domains
            .linked
            .values()
            .chain(cfg.domains.proxy.values())
            .flat_map(|delta| delta.added.iter().map(|d| (None, d)));
        let parked =
            cfg.domains.parked.iter().flat_map(|(root, delta)| {
                delta.added.iter().map(move |d| (Some(root.as_str()), d))
            });
        Self {
            linked: cfg.linked.iter().map(yerd_core::Site::name).collect(),
            proxies: cfg.proxies.iter().map(yerd_core::ProxySite::name).collect(),
            added: unowned
                .chain(parked)
                .filter(|(_, d)| !d.is_wildcard())
                .map(|(owner, d)| (owner, d.as_str()))
                .collect(),
        }
    }

    fn added_by_other(&self, name: &str, doc_root: &str) -> bool {
        self.added
            .iter()
            .any(|(owner, domain)| *domain == name && *owner != Some(doc_root))
    }

    fn blocks(&self, name: &str, doc_root: &str) -> bool {
        self.linked.contains(name)
            || self.proxies.contains(name)
            || self.added_by_other(name, doc_root)
    }
}

/// Name every parked candidate, in the given root-major order, or `None` to
/// skip it. Each folder's legacy name is [`yerd_core::normalize_site_name`]
/// (what builds before suffix stripping served it as) and its preferred name is
/// [`yerd_core::parked_folder_site_name`]. In order:
///
/// 1. A folder holding a claim in `claims` (see
///    [`yerd_config::ParkedSection::claims`]) for its current preferred name
///    keeps it first, unless a linked site, proxy or another claimant's
///    explicitly added domain holds that name (the claim is then ignored).
/// 2. Folders sharing a legacy name keep only the first, claimants first;
///    later ones are skipped, exactly as the router dropped such duplicates
///    before.
/// 3. The survivors' legacy names are reserved, so no stripped name can take
///    another folder's fallback slug. A folder's own reservation never blocks
///    its own fallback.
/// 4. A folder whose name does not change keeps it, unless a linked site holds
///    it (linked wins, as before) or another folder claims it (the newcomer is
///    hidden, with a warning).
/// 5. A stripped name wanted by two or more unclaimed changed folders is
///    contested and given to none of them (`laravel.com` and `laravel.org` both
///    fall back), so adding a folder never hands an existing site's name and
///    settings to a different project.
/// 6. Each remaining folder takes its stripped name if it is uncontested and no
///    linked site, proxy, other folder's explicitly added domain, reserved
///    legacy name, claim or earlier pick holds it; otherwise its legacy name,
///    unless a linked site, claim or earlier pick holds that too.
///
/// The result depends only on the folder set and config, so it is stable
/// across rescans and restarts.
fn assign_parked_names(
    candidates: &[ParkedCandidate<'_>],
    taken: &TakenNames<'_>,
    claims: &BTreeMap<String, yerd_config::ParkedClaim>,
) -> Vec<Option<String>> {
    let names: Vec<(Option<String>, Option<String>)> = candidates
        .iter()
        .map(|candidate| {
            let legacy = yerd_core::normalize_site_name(candidate.folder);
            let preferred = legacy
                .as_ref()
                .and_then(|_| yerd_core::parked_folder_site_name(candidate.folder));
            (legacy, preferred)
        })
        .collect();

    let held = held_claims(candidates, &names, taken, claims);
    let winners: HashSet<usize> = held.values().copied().collect();

    let mut seen: HashSet<&str> = HashSet::new();
    for (index, (legacy, _)) in names.iter().enumerate() {
        if let (true, Some(legacy)) = (winners.contains(&index), legacy.as_deref()) {
            seen.insert(legacy);
        }
    }
    let alive: Vec<bool> = names
        .iter()
        .enumerate()
        .map(|(index, (legacy, _))| {
            winners.contains(&index) || legacy.as_deref().is_some_and(|l| seen.insert(l))
        })
        .collect();
    let reserved: HashSet<&str> = names
        .iter()
        .zip(&alive)
        .filter(|(_, alive)| **alive)
        .filter_map(|((legacy, _), _)| legacy.as_deref())
        .collect();
    let mut wanted: HashMap<&str, usize> = HashMap::new();
    for (index, ((legacy, preferred), alive)) in names.iter().zip(&alive).enumerate() {
        if let (true, Some(legacy), Some(preferred)) = (*alive, legacy, preferred.as_deref()) {
            if preferred != legacy && !winners.contains(&index) {
                *wanted.entry(preferred).or_default() += 1;
            }
        }
    }

    let mut out: Vec<Option<String>> = vec![None; names.len()];
    for (name, index) in &held {
        if let Some(slot) = out.get_mut(*index) {
            *slot = Some(name.clone());
        }
    }
    let mut picked: HashSet<String> = held.keys().cloned().collect();
    let open: Vec<bool> = alive
        .iter()
        .enumerate()
        .map(|(index, alive)| *alive && !winners.contains(&index))
        .collect();
    for (((slot, (legacy, preferred)), candidate), open) in
        out.iter_mut().zip(&names).zip(candidates).zip(&open)
    {
        let (true, Some(legacy)) = (*open, legacy.as_deref()) else {
            continue;
        };
        if preferred.as_deref() != Some(legacy) || taken.linked.contains(legacy) {
            continue;
        }
        if let Some(holder) = held.get(legacy).and_then(|index| candidates.get(*index)) {
            tracing::warn!(
                path = %candidate.doc_root,
                claimed_by = %holder.doc_root,
                name = %legacy,
                "hiding parked folder: its site name is claimed by another parked folder"
            );
            continue;
        }
        picked.insert(legacy.to_owned());
        *slot = Some(legacy.to_owned());
    }
    for (((slot, (legacy, preferred)), candidate), open) in
        out.iter_mut().zip(&names).zip(candidates).zip(&open)
    {
        let (true, Some(legacy)) = (*open, legacy.as_deref()) else {
            continue;
        };
        if preferred.as_deref() == Some(legacy) {
            continue;
        }
        let pick = preferred.as_deref().filter(|p| {
            wanted.get(p).copied() == Some(1)
                && !taken.blocks(p, candidate.doc_root)
                && !reserved.contains(p)
                && !picked.contains(*p)
        });
        let fallback = Some(legacy).filter(|l| !taken.linked.contains(l) && !picked.contains(*l));
        if let Some(name) = pick.or(fallback) {
            picked.insert(name.to_owned());
            *slot = Some(name.to_owned());
        }
    }
    out
}

/// The claimed names that hold, mapped to the index of the candidate holding
/// each: a candidate's claim counts when it names the candidate's current
/// stripped name and no linked site, proxy or other claimant's added domain
/// holds that name. The first candidate wins a (corrupt) duplicate claim.
fn held_claims(
    candidates: &[ParkedCandidate<'_>],
    names: &[(Option<String>, Option<String>)],
    taken: &TakenNames<'_>,
    claims: &BTreeMap<String, yerd_config::ParkedClaim>,
) -> HashMap<String, usize> {
    let mut held: HashMap<String, usize> = HashMap::new();
    for (index, (candidate, (legacy, preferred))) in candidates.iter().zip(names).enumerate() {
        let (Some(legacy), Some(preferred)) = (legacy, preferred) else {
            continue;
        };
        let claimed = claims
            .get(candidate.doc_root)
            .is_some_and(|claim| claim.name == *preferred);
        if claimed && preferred != legacy && !taken.blocks(preferred, candidate.doc_root) {
            held.entry(preferred.clone()).or_insert(index);
        }
    }
    held
}

/// A parked site whose name changed when public-suffix stripping arrived:
/// `legacy` is the name older builds served it under (`laravel-com`), `new`
/// the name it is served under now (`laravel`), and `doc_root` its
/// document-root key, the same `to_string_lossy` string the overrides and
/// per-site maps use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParkedRename {
    legacy: String,
    new: String,
    pub(crate) doc_root: String,
}

/// Every parked site in `router` served under a name other than its legacy
/// folder name and not yet holding a claim for it, skipping any whose legacy
/// name is still a live site (that site owns the old settings). These are the
/// sites renamed by upgrading to suffix stripping.
fn parked_renames(
    router: &SiteRouter,
    claims: &BTreeMap<String, yerd_config::ParkedClaim>,
) -> Vec<ParkedRename> {
    router
        .iter()
        .filter(|site| site.kind() == yerd_core::SiteKind::Parked)
        .filter_map(|site| {
            let folder = site.document_root().file_name()?.to_str()?;
            let legacy = yerd_core::normalize_site_name(folder)?;
            let doc_root = site.document_root().to_string_lossy().into_owned();
            let claimed = claims
                .get(&doc_root)
                .is_some_and(|claim| claim.name == site.name());
            (legacy != site.name() && !claimed && router.get(&legacy).is_none()).then(|| {
                ParkedRename {
                    legacy,
                    new: site.name().to_owned(),
                    doc_root,
                }
            })
        })
        .collect()
}

/// The parked sites renamed while the daemon runs: those [`parked_renames`]
/// finds in `router` that `previous` served from the same folder under their
/// legacy name (a contested name was freed, or a linked site or proxy holding
/// it went away). A folder that is new to `router` inherits nothing.
pub(crate) fn live_parked_renames(
    previous: &SiteRouter,
    router: &SiteRouter,
    claims: &BTreeMap<String, yerd_config::ParkedClaim>,
) -> Vec<ParkedRename> {
    parked_renames(router, claims)
        .into_iter()
        .filter(|rename| {
            previous.get(&rename.legacy).is_some_and(|site| {
                site.kind() == yerd_core::SiteKind::Parked
                    && site.document_root().to_string_lossy() == rename.doc_root.as_str()
            })
        })
        .collect()
}

/// The URL syncs `renames` need, for [`sync_renamed_site_urls`].
pub(crate) fn renamed_sites(renames: &[ParkedRename]) -> Vec<RenamedSite> {
    renames
        .iter()
        .map(|rename| RenamedSite {
            name: rename.new.clone(),
            doc_root: rename.doc_root.clone(),
        })
        .collect()
}

/// Bring [`yerd_config::ParkedSection::claims`] in line with `router`: every
/// parked site served under a name other than its legacy folder name holds a
/// claim for it, and every other claim is dropped (its folder is gone, or the
/// site is no longer served under that name), except a claim whose URL sync is
/// still pending while its folder is missing (an unmounted drive), which is
/// kept so the sync resumes when the folder returns. A new claim is created with
/// `url_synced = false` when its document root is in `pending_url_sync` (a site
/// renamed on upgrade) and `true` otherwise. Returns whether the claims
/// changed, so the caller knows to save.
pub(crate) fn reconcile_parked_claims(
    cfg: &mut yerd_config::Config,
    router: &SiteRouter,
    pending_url_sync: &HashSet<&str>,
) -> bool {
    let served: BTreeMap<String, String> = router
        .iter()
        .filter(|site| site.kind() == yerd_core::SiteKind::Parked)
        .filter_map(|site| {
            let folder = site.document_root().file_name()?.to_str()?;
            let legacy = yerd_core::normalize_site_name(folder)?;
            (legacy != site.name()).then(|| {
                (
                    site.document_root().to_string_lossy().into_owned(),
                    site.name().to_owned(),
                )
            })
        })
        .collect();
    let before = cfg.parked.claims.clone();
    cfg.parked.claims.retain(|doc_root, claim| {
        served.get(doc_root) == Some(&claim.name)
            || (!claim.url_synced && !Path::new(doc_root).is_dir())
    });
    for (doc_root, name) in served {
        let url_synced = !pending_url_sync.contains(doc_root.as_str());
        cfg.parked
            .claims
            .entry(doc_root)
            .or_insert(yerd_config::ParkedClaim { name, url_synced });
    }
    cfg.parked.claims != before
}

/// Bring [`yerd_config::AddedSection::parked`] in line with `router`: every
/// served parked site without an entry is recorded as first seen at `now`, or
/// as `0` (undated) when its parked root is still listed in
/// [`yerd_config::AddedSection::baseline_pending`].
///
/// A parked root counts as available once the router serves at least one site
/// under it. Only an available root leaves the baseline list, and only an
/// available root has its entries pruned, and then only those whose folder is
/// gone (`is_dir` false): a folder hidden by a same-named linked site keeps its
/// time. An unavailable root (a missing drive, or on Linux an empty mount point)
/// keeps everything, so its sites are neither re-stamped nor lose their times
/// when it returns. Returns whether `[added]` changed, so the caller knows to
/// save.
pub(crate) fn reconcile_parked_added(
    cfg: &mut yerd_config::Config,
    router: &SiteRouter,
    now: u64,
    is_dir: impl Fn(&str) -> bool,
) -> bool {
    let served: HashSet<String> = router
        .iter()
        .filter(|site| site.kind() == yerd_core::SiteKind::Parked)
        .map(|site| site.document_root().to_string_lossy().into_owned())
        .collect();
    let parked_roots = &cfg.parked.paths;
    let root_of = |doc_root: &str| -> Option<String> {
        Path::new(doc_root)
            .parent()
            .and_then(Path::to_str)
            .filter(|parent| parked_roots.contains(*parent))
            .map(str::to_owned)
    };
    let available: HashSet<String> = served.iter().filter_map(|r| root_of(r)).collect();
    let before = cfg.added.clone();
    cfg.added.parked.retain(|doc_root, _| {
        served.contains(doc_root)
            || root_of(doc_root).is_some_and(|root| !available.contains(&root) || is_dir(doc_root))
    });
    for doc_root in served {
        let pending =
            root_of(&doc_root).is_some_and(|root| cfg.added.baseline_pending.contains(&root));
        cfg.added
            .parked
            .entry(doc_root)
            .or_insert(if pending { 0 } else { now });
    }
    cfg.added
        .baseline_pending
        .retain(|root| parked_roots.contains(root) && !available.contains(root));
    cfg.added != before
}

/// Rekey every name-keyed setting from each rename's legacy name to its new
/// one, never overwriting an entry already under the new name. Covers
/// favourites, group membership, tunnel hostnames and passwords (including the
/// older `<name>.<tld>` key form), per-site service instances, and the apex
/// held in the site's parked domain delta (`suppressed` and `primary`; `added`
/// domains are left so those hosts keep routing). Returns whether anything
/// changed, so a second run is a no-op.
pub(crate) fn rekey_parked_names(cfg: &mut yerd_config::Config, renames: &[ParkedRename]) -> bool {
    let mut changed = false;
    for rename in renames {
        if rekey_parked_name(cfg, rename) {
            tracing::info!(
                from = %rename.legacy,
                to = %rename.new,
                "moved parked site settings to its suffix-stripped name"
            );
            changed = true;
        }
    }
    changed
}

fn rekey_parked_name(cfg: &mut yerd_config::Config, rename: &ParkedRename) -> bool {
    let mut changed = rekey_parked_domains(cfg, rename);
    if cfg.favourites.sites.remove(&rename.legacy) {
        cfg.favourites.sites.insert(rename.new.clone());
        changed = true;
    }
    changed |= move_key(
        &mut cfg.groups.members,
        "group",
        &rename.legacy,
        &rename.new,
    );
    let host_key = format!("{}.{}", rename.legacy, cfg.tld.as_str());
    for key in [rename.legacy.as_str(), host_key.as_str()] {
        changed |= move_key(&mut cfg.tunnel.sites, "tunnel hostname", key, &rename.new);
        changed |= move_key(
            &mut cfg.tunnel.passwords,
            "tunnel password",
            key,
            &rename.new,
        );
    }
    changed | rekey_service_instances(&mut cfg.services.instances, rename)
}

fn rekey_parked_domains(cfg: &mut yerd_config::Config, rename: &ParkedRename) -> bool {
    let Some(delta) = cfg.domains.parked.get_mut(&rename.doc_root) else {
        return false;
    };
    let old = yerd_core::Domain::apex(&rename.legacy);
    let new = yerd_core::Domain::apex(&rename.new);
    let mut changed = false;
    for domain in delta.suppressed.iter_mut().chain(delta.primary.iter_mut()) {
        if *domain == old {
            domain.clone_from(&new);
            changed = true;
        }
    }
    changed
}

fn rekey_service_instances(
    instances: &mut BTreeMap<String, yerd_config::ServiceInstance>,
    rename: &ParkedRename,
) -> bool {
    let keys: Vec<String> = instances
        .keys()
        .filter(|key| {
            crate::services::parse_wire_id(key).1.as_deref() == Some(rename.legacy.as_str())
        })
        .cloned()
        .collect();
    let mut changed = false;
    for key in keys {
        let (type_id, _) = crate::services::parse_wire_id(&key);
        let new_key = crate::services::wire_id(&type_id, Some(&rename.new));
        if instances.contains_key(&new_key) {
            tracing::warn!(
                setting = "service instance",
                from = %key,
                to = %new_key,
                "not moving a renamed parked site's setting: the new name already has one"
            );
            continue;
        }
        if let Some(mut instance) = instances.remove(&key) {
            instance.site = Some(rename.new.clone());
            instances.insert(new_key, instance);
            changed = true;
        }
    }
    changed
}

fn move_key<V>(map: &mut BTreeMap<String, V>, setting: &str, from: &str, to: &str) -> bool {
    if !map.contains_key(from) {
        return false;
    }
    if map.contains_key(to) {
        tracing::warn!(
            setting,
            from,
            to,
            "not moving a renamed parked site's setting: the new name already has one"
        );
        return false;
    }
    match map.remove(from) {
        Some(value) => {
            map.insert(to.to_owned(), value);
            true
        }
        None => false,
    }
}

/// A parked site renamed by public-suffix stripping whose app-side URL
/// (`WordPress` `siteurl`/`home`, `CodeIgniter` `app.baseURL`) has not yet been
/// pointed at its new name: a claim with `url_synced = false`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenamedSite {
    /// The site's current name.
    pub name: String,
    /// The site's document-root key.
    pub doc_root: String,
}

/// Every claim whose one-time URL sync is still pending.
fn pending_url_syncs(cfg: &yerd_config::Config) -> Vec<RenamedSite> {
    cfg.parked
        .claims
        .iter()
        .filter(|(_, claim)| !claim.url_synced)
        .map(|(doc_root, claim)| RenamedSite {
            name: claim.name.clone(),
            doc_root: doc_root.clone(),
        })
        .collect()
}

/// Point each renamed site's app-side URL at its new name, mirroring the
/// post-mutation hook. Run after the autostarted services are up, because the
/// `WordPress` update goes through WP-CLI and needs the site's database. A site
/// is only synced while the router still serves that name from the same parked
/// folder, and is marked done (`url_synced = true`, saved) only when no hook
/// failed; anything else stays pending and is retried on the next start, which
/// is safe because it re-sets the same URL. Stops as soon as shutdown is
/// requested, and skipped while the web ports are unbound (a URL written then
/// would lack the listener's port).
pub(crate) async fn sync_renamed_site_urls(state: &DaemonState, renamed: &[RenamedSite]) {
    if state.web_unbound.is_some() {
        tracing::debug!("web ports unbound; renamed sites' URL sync stays pending");
        return;
    }
    let shutdown = state.shutdown_tx.subscribe();
    for renamed_site in renamed {
        if *shutdown.borrow() {
            return;
        }
        let site = state
            .router
            .read()
            .await
            .get(&renamed_site.name)
            .filter(|site| {
                site.kind() == yerd_core::SiteKind::Parked
                    && site.document_root().to_string_lossy() == renamed_site.doc_root.as_str()
            })
            .cloned();
        let Some(site) = site else {
            tracing::debug!(site = %renamed_site.name, "renamed site not served from its folder; URL sync stays pending");
            continue;
        };
        let wordpress = crate::wordpress_url_sync::sync_site_url(&site, state).await;
        let codeigniter = crate::codeigniter_url_sync::sync_base_url(&site, state).await;
        if [wordpress, codeigniter].contains(&crate::public_url::UrlSyncOutcome::Failed) {
            continue;
        }
        mark_url_synced(state, renamed_site).await;
    }
}

async fn mark_url_synced(state: &DaemonState, renamed_site: &RenamedSite) {
    let mut cfg = state.config.lock().await;
    let Some(claim) = cfg.parked.claims.get_mut(&renamed_site.doc_root) else {
        return;
    };
    if claim.name != renamed_site.name || claim.url_synced {
        return;
    }
    claim.url_synced = true;
    if let Err(e) = cfg.save(&state.config_path) {
        tracing::warn!(error = %e, "could not persist a renamed site's URL sync");
    }
}

/// Build a parked [`Site`] for `doc_root`, re-applying any persisted per-site
/// override (kept parked - no promotion to linked) and resolving its web root.
/// A manual `web_root` override pins it; otherwise it auto-detects, and an
/// unresolved detection serves the root provisionally and pushes `doc_root` onto
/// `watch_roots`. Returns `None` (logging) for an invalid site name.
fn build_parked_site(
    site_name: &str,
    doc_root: &std::path::Path,
    default_php: PhpVersion,
    cfg: &yerd_config::Config,
    detect_cache: &DetectCache,
    watch_roots: &mut Vec<PathBuf>,
) -> Option<Site> {
    let mut site = match Site::parked(site_name, doc_root, default_php) {
        Ok(site) => site,
        Err(e) => {
            tracing::debug!(
                name = %site_name,
                error = %e,
                "skipping invalid parked-site name"
            );
            return None;
        }
    };

    let key = doc_root.to_string_lossy().into_owned();
    let ov = cfg.overrides.get(&key);
    if let Some(ov) = ov {
        if let Some(php) = ov.php {
            site.set_php(php);
        }
        if let Some(secure) = ov.secure {
            site.set_secure(secure);
        }
        if let Some(wp_auto_login) = ov.wp_auto_login {
            site.set_wp_auto_login(wp_auto_login);
        }
        if ov.wp_auto_login_user.is_some() {
            site.set_wp_auto_login_user(ov.wp_auto_login_user.clone());
        }
        if ov.front_controller.is_some() {
            site.set_front_controller(ov.front_controller);
        }
    }

    if let Some(rel) = ov.and_then(|o| o.web_root.as_deref()) {
        site.set_web_subpath(rel);
    } else {
        let det = detect_cache.detect(doc_root);
        site.set_web_subpath(det.subpath);
        if !det.resolved {
            watch_roots.push(doc_root.to_path_buf());
        }
    }

    Some(site)
}

/// A per-process id clients use to detect that a restart actually completed.
/// Derived from the pid + wall-clock nanos (which differ across restarts), so
/// it changes even though the in-place re-exec preserves the pid. Needs no RNG
/// dependency; collisions are irrelevant - only a *change* is observed.
fn rand_boot_id() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::process::id().hash(&mut h);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
        .hash(&mut h);
    // Mask to 52 bits so the value is always exactly representable as a JS
    // double (the GUI compares it after `JSON.parse`); above 2^53 precision is
    // lost and two distinct ids could round equal.
    h.finish() & ((1u64 << 52) - 1)
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
        std::fs::create_dir_all(parked_root.join(".hidden")).unwrap();

        let mut cfg = yerd_config::Config::default();
        cfg.parked
            .paths
            .insert(parked_root.to_string_lossy().into_owned());

        let dirs = make_dirs(tmp.path());
        let (sites, _) =
            scan_sites(&cfg, PhpVersion::new(8, 3), &dirs, &DetectCache::new()).unwrap();
        let mut names: Vec<&str> = sites.iter().map(yerd_core::Site::name).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["app1", "app2"]);
    }

    #[test]
    fn scan_sites_slugifies_underscored_parked_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let parked_root = tmp.path().join("Sites");
        std::fs::create_dir_all(parked_root.join("foo_bar")).unwrap();

        let mut cfg = yerd_config::Config::default();
        cfg.parked
            .paths
            .insert(parked_root.to_string_lossy().into_owned());

        let dirs = make_dirs(tmp.path());
        let (sites, _) =
            scan_sites(&cfg, PhpVersion::new(8, 3), &dirs, &DetectCache::new()).unwrap();
        let names: Vec<&str> = sites.iter().map(yerd_core::Site::name).collect();
        assert_eq!(names, vec!["foo-bar"]);
    }

    #[test]
    fn scan_sites_detects_web_root_and_collects_unresolved() {
        let tmp = tempfile::tempdir().unwrap();
        let parked_root = tmp.path().join("Sites");
        let laravel = parked_root.join("app");
        std::fs::create_dir_all(laravel.join("public")).unwrap();
        std::fs::write(laravel.join("artisan"), b"").unwrap();
        std::fs::write(laravel.join("public/index.php"), b"").unwrap();
        std::fs::create_dir_all(parked_root.join("empty")).unwrap();

        let mut cfg = yerd_config::Config::default();
        cfg.parked
            .paths
            .insert(parked_root.to_string_lossy().into_owned());
        let dirs = make_dirs(tmp.path());
        let (sites, watch_roots) =
            scan_sites(&cfg, PhpVersion::new(8, 3), &dirs, &DetectCache::new()).unwrap();

        let app = sites.iter().find(|s| s.name() == "app").unwrap();
        assert_eq!(app.web_subpath(), std::path::Path::new("public"));
        let empty = sites.iter().find(|s| s.name() == "empty").unwrap();
        assert_eq!(empty.web_subpath(), std::path::Path::new(""));
        assert_eq!(watch_roots, vec![parked_root.join("empty")]);
    }

    #[test]
    fn scan_sites_web_root_override_pins_and_skips_watching() {
        let tmp = tempfile::tempdir().unwrap();
        let parked_root = tmp.path().join("Sites");
        std::fs::create_dir_all(parked_root.join("app")).unwrap();
        let mut cfg = yerd_config::Config::default();
        cfg.parked
            .paths
            .insert(parked_root.to_string_lossy().into_owned());
        cfg.overrides.insert(
            parked_root.join("app").to_string_lossy().into_owned(),
            yerd_config::SiteOverride {
                php: None,
                secure: None,
                web_root: Some("public".to_string()),
                wp_auto_login: None,
                wp_auto_login_user: None,
                front_controller: None,
            },
        );
        let dirs = make_dirs(tmp.path());
        let (sites, watch_roots) =
            scan_sites(&cfg, PhpVersion::new(8, 3), &dirs, &DetectCache::new()).unwrap();
        let app = sites.iter().find(|s| s.name() == "app").unwrap();
        assert_eq!(app.web_subpath(), std::path::Path::new("public"));
        assert!(watch_roots.is_empty());
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
        let (sites, _) =
            scan_sites(&cfg, PhpVersion::new(8, 3), &dirs, &DetectCache::new()).unwrap();
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
        let (sites, _) =
            scan_sites(&cfg, PhpVersion::new(8, 3), &dirs, &DetectCache::new()).unwrap();
        assert_eq!(sites.len(), 1);
        assert_eq!(
            sites[0].document_root(),
            tmp.path().join("linked-collide").as_path()
        );
    }

    /// Build a config with `Sites/blog` parked and an override keyed by blog's
    /// document_root (the same string scan_sites computes).
    fn cfg_with_parked_blog_override(
        tmp: &std::path::Path,
        ov: yerd_config::SiteOverride,
    ) -> yerd_config::Config {
        let parked_root = tmp.join("Sites");
        std::fs::create_dir_all(parked_root.join("blog")).unwrap();
        let mut cfg = yerd_config::Config::default();
        cfg.parked
            .paths
            .insert(parked_root.to_string_lossy().into_owned());
        let key = parked_root.join("blog").to_string_lossy().into_owned();
        cfg.overrides.insert(key, ov);
        cfg
    }

    #[test]
    fn scan_sites_applies_php_override() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_with_parked_blog_override(
            tmp.path(),
            yerd_config::SiteOverride {
                php: Some(PhpVersion::new(8, 5)),
                secure: None,
                web_root: None,
                wp_auto_login: None,
                wp_auto_login_user: None,
                front_controller: None,
            },
        );
        let dirs = make_dirs(tmp.path());
        let (sites, _) =
            scan_sites(&cfg, PhpVersion::new(8, 3), &dirs, &DetectCache::new()).unwrap();
        let blog = sites.iter().find(|s| s.name() == "blog").unwrap();
        assert_eq!(blog.php(), PhpVersion::new(8, 5));
        assert!(!blog.secure());
        assert_eq!(blog.kind(), yerd_core::SiteKind::Parked);
    }

    #[test]
    fn scan_sites_applies_secure_override() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_with_parked_blog_override(
            tmp.path(),
            yerd_config::SiteOverride {
                php: None,
                secure: Some(true),
                web_root: None,
                wp_auto_login: None,
                wp_auto_login_user: None,
                front_controller: None,
            },
        );
        let dirs = make_dirs(tmp.path());
        let (sites, _) =
            scan_sites(&cfg, PhpVersion::new(8, 3), &dirs, &DetectCache::new()).unwrap();
        let blog = sites.iter().find(|s| s.name() == "blog").unwrap();
        assert!(blog.secure());
        assert_eq!(blog.php(), PhpVersion::new(8, 3));
        assert_eq!(blog.kind(), yerd_core::SiteKind::Parked);
    }

    #[test]
    fn scan_sites_orphan_override_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let parked_root = tmp.path().join("Sites");
        std::fs::create_dir_all(parked_root.join("blog")).unwrap();
        let mut cfg = yerd_config::Config::default();
        cfg.parked
            .paths
            .insert(parked_root.to_string_lossy().into_owned());
        cfg.overrides.insert(
            parked_root.join("ghost").to_string_lossy().into_owned(),
            yerd_config::SiteOverride {
                php: Some(PhpVersion::new(8, 5)),
                secure: Some(true),
                web_root: None,
                wp_auto_login: None,
                wp_auto_login_user: None,
                front_controller: None,
            },
        );
        let dirs = make_dirs(tmp.path());
        let (sites, _) =
            scan_sites(&cfg, PhpVersion::new(8, 3), &dirs, &DetectCache::new()).unwrap();
        let blog = sites.iter().find(|s| s.name() == "blog").unwrap();
        assert_eq!(blog.php(), PhpVersion::new(8, 3));
        assert!(!blog.secure());
    }

    #[test]
    fn scan_sites_linked_collision_leaves_override_dormant() {
        let tmp = tempfile::tempdir().unwrap();
        let parked_root = tmp.path().join("Sites");
        std::fs::create_dir_all(parked_root.join("blog")).unwrap();
        let linked =
            Site::linked("blog", tmp.path().join("real-blog"), PhpVersion::new(7, 4)).unwrap();
        let mut cfg = yerd_config::Config::default();
        cfg.linked.push(linked);
        cfg.parked
            .paths
            .insert(parked_root.to_string_lossy().into_owned());
        cfg.overrides.insert(
            parked_root.join("blog").to_string_lossy().into_owned(),
            yerd_config::SiteOverride {
                php: Some(PhpVersion::new(8, 5)),
                secure: Some(true),
                web_root: None,
                wp_auto_login: None,
                wp_auto_login_user: None,
                front_controller: None,
            },
        );
        let dirs = make_dirs(tmp.path());
        let (sites, _) =
            scan_sites(&cfg, PhpVersion::new(8, 3), &dirs, &DetectCache::new()).unwrap();
        let blog = sites.iter().find(|s| s.name() == "blog").unwrap();
        assert_eq!(blog.kind(), yerd_core::SiteKind::Linked);
        assert_eq!(blog.php(), PhpVersion::new(7, 4));
        assert!(!blog.secure());
    }

    #[test]
    fn rand_boot_id_fits_52_bits() {
        let id = rand_boot_id();
        assert!(id < (1u64 << 52), "boot id {id} exceeds 52 bits");
    }

    #[test]
    fn build_router_and_routing_empty_config_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let cfg = yerd_config::Config::default();
        let cache = DetectCache::new();
        let (router, wordpress_sites, laravel_sites, framework_sites) =
            build_router(&cfg, &dirs, &cache).unwrap();
        assert!(router.is_empty());
        assert!(wordpress_sites.is_empty());
        assert!(laravel_sites.is_empty());
        assert!(framework_sites.is_empty());
        let (router2, wordpress_sites2, laravel_sites2, framework_sites2, watch_roots) =
            build_routing(&cfg, &dirs, &cache).unwrap();
        assert!(router2.is_empty());
        assert!(wordpress_sites2.is_empty());
        assert!(laravel_sites2.is_empty());
        assert!(framework_sites2.is_empty());
        assert!(watch_roots.is_empty());
    }

    #[test]
    fn build_routing_includes_parked_site() {
        let tmp = tempfile::tempdir().unwrap();
        let parked_root = tmp.path().join("Sites");
        std::fs::create_dir_all(parked_root.join("shop")).unwrap();
        let mut cfg = yerd_config::Config::default();
        cfg.parked
            .paths
            .insert(parked_root.to_string_lossy().into_owned());
        let dirs = make_dirs(tmp.path());
        let (router, wordpress_sites, _laravel_sites, framework_sites, watch_roots) =
            build_routing(&cfg, &dirs, &DetectCache::new()).unwrap();
        assert!(!router.is_empty());
        assert_eq!(wordpress_sites.get("shop"), Some(&false));
        assert!(framework_sites.is_empty());
        assert_eq!(watch_roots, vec![parked_root.join("shop")]);
    }

    #[test]
    fn build_routing_detects_wordpress_site() {
        let tmp = tempfile::tempdir().unwrap();
        let parked_root = tmp.path().join("Sites");
        std::fs::create_dir_all(parked_root.join("blog")).unwrap();
        std::fs::write(parked_root.join("blog").join("wp-config.php"), b"<?php").unwrap();
        let mut cfg = yerd_config::Config::default();
        cfg.parked
            .paths
            .insert(parked_root.to_string_lossy().into_owned());
        let dirs = make_dirs(tmp.path());
        let (router, wordpress_sites, _laravel_sites, _framework_sites, _watch_roots) =
            build_routing(&cfg, &dirs, &DetectCache::new()).unwrap();
        assert!(!router.is_empty());
        assert_eq!(wordpress_sites.get("blog"), Some(&true));
    }

    #[test]
    fn build_routing_detects_framework() {
        let tmp = tempfile::tempdir().unwrap();
        let parked_root = tmp.path().join("Sites");
        std::fs::create_dir_all(parked_root.join("cms")).unwrap();
        std::fs::write(
            parked_root.join("cms").join("composer.json"),
            br#"{"require":{"laravel/framework":"^11","statamic/cms":"^5"}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(parked_root.join("blog")).unwrap();
        std::fs::write(parked_root.join("blog").join("wp-config.php"), b"<?php").unwrap();
        std::fs::create_dir_all(parked_root.join("plain")).unwrap();
        let mut cfg = yerd_config::Config::default();
        cfg.parked
            .paths
            .insert(parked_root.to_string_lossy().into_owned());
        let dirs = make_dirs(tmp.path());
        let (_router, _wordpress_sites, _laravel_sites, framework_sites, _watch_roots) =
            build_routing(&cfg, &dirs, &DetectCache::new()).unwrap();
        assert_eq!(
            framework_sites.get("cms"),
            Some(&yerd_core::SiteFramework::Statamic)
        );
        assert_eq!(
            framework_sites.get("blog"),
            Some(&yerd_core::SiteFramework::Wordpress)
        );
        assert_eq!(framework_sites.get("plain"), None);
    }

    #[test]
    fn parked_candidate_filters_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("MyApp")).unwrap();
        std::fs::create_dir_all(root.join("ex.com")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        std::fs::write(root.join("afile"), b"x").unwrap();

        let mut got: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::default();
        for entry in std::fs::read_dir(root).unwrap().flatten() {
            let key = entry.file_name().to_string_lossy().into_owned();
            got.insert(key, parked_candidate(&entry));
        }
        assert_eq!(got.get("MyApp").unwrap().as_deref(), Some("MyApp"));
        assert_eq!(got.get("ex.com").unwrap().as_deref(), Some("ex.com"));
        assert_eq!(got.get(".hidden").unwrap().as_deref(), None);
        assert_eq!(got.get("afile").unwrap().as_deref(), None);
    }

    type AssignCase = (
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
        &'static [&'static str],
        &'static [Option<&'static str>],
    );

    const ASSIGN_CASES: &[AssignCase] = &[
        (
            "suffix stripped",
            &["laravel.com"],
            &[],
            &[],
            &[Some("laravel")],
        ),
        (
            "plain folder keeps its name, suffixed one falls back",
            &["laravel", "laravel.com"],
            &[],
            &[],
            &[Some("laravel"), Some("laravel-com")],
        ),
        (
            "a contested stripped name goes to nobody",
            &["laravel.com", "laravel.org"],
            &[],
            &[],
            &[Some("laravel-com"), Some("laravel-org")],
        ),
        (
            "a three-way contest also goes to nobody",
            &["laravel.app", "laravel.com", "laravel.org"],
            &[],
            &[],
            &[
                Some("laravel-app"),
                Some("laravel-com"),
                Some("laravel-org"),
            ],
        ),
        (
            "uncontested stripped names are unaffected by a contest",
            &["acme.io", "laravel.com", "laravel.org"],
            &[],
            &[],
            &[Some("acme"), Some("laravel-com"), Some("laravel-org")],
        ),
        (
            "unchanged folder wins regardless of sort",
            &["a.com", "a_"],
            &[],
            &[],
            &[Some("a-com"), Some("a")],
        ),
        (
            "reserved legacy names block stripped picks",
            &["a", "a-com.org", "a.com"],
            &[],
            &[],
            &[Some("a"), Some("a-com-org"), Some("a-com")],
        ),
        (
            "duplicate plain names across roots keep the first",
            &["blog", "blog"],
            &[],
            &[],
            &[Some("blog"), None],
        ),
        (
            "duplicate suffixed names across roots keep the first",
            &["laravel.com", "laravel.com"],
            &[],
            &[],
            &[Some("laravel"), None],
        ),
        (
            "later plain folder sharing the legacy name is skipped",
            &["laravel.com", "laravel-com"],
            &[],
            &[],
            &[Some("laravel"), None],
        ),
        (
            "stripped name held by a linked site falls back",
            &["laravel.com"],
            &["laravel"],
            &[],
            &[Some("laravel-com")],
        ),
        (
            "legacy name held by a linked site is skipped",
            &["shop"],
            &["shop"],
            &[],
            &[None],
        ),
        (
            "stripped and legacy both held by linked sites",
            &["laravel.com"],
            &["laravel", "laravel-com"],
            &[],
            &[None],
        ),
        (
            "stripped name held by a proxy falls back",
            &["reverb.app"],
            &[],
            &["reverb"],
            &[Some("reverb-app")],
        ),
        (
            "plain names unchanged",
            &["MyApp", "foo_bar", "keep--me", "???"],
            &[],
            &[],
            &[Some("myapp"), Some("foo-bar"), Some("keep--me"), None],
        ),
    ];

    type AddedDomain<'a> = (Option<&'a str>, &'a str);

    fn doc_root_for(index: usize, folder: &str) -> String {
        format!("/root{index}/{folder}")
    }

    fn assign<'a>(
        folders: &[&str],
        linked: &[&'a str],
        proxies: &[&'a str],
        added: Vec<(Option<&'a str>, &'a str)>,
        claimed: &[(usize, &str)],
    ) -> Vec<Option<String>> {
        let doc_roots: Vec<String> = folders
            .iter()
            .enumerate()
            .map(|(i, f)| doc_root_for(i, f))
            .collect();
        let candidates: Vec<ParkedCandidate<'_>> = folders
            .iter()
            .zip(&doc_roots)
            .map(|(folder, doc_root)| ParkedCandidate { folder, doc_root })
            .collect();
        let taken = TakenNames {
            linked: linked.iter().copied().collect(),
            proxies: proxies.iter().copied().collect(),
            added,
        };
        let claims: BTreeMap<String, yerd_config::ParkedClaim> = claimed
            .iter()
            .filter_map(|(index, name)| {
                folders.get(*index).map(|folder| {
                    (
                        doc_root_for(*index, folder),
                        yerd_config::ParkedClaim {
                            name: (*name).to_owned(),
                            url_synced: true,
                        },
                    )
                })
            })
            .collect();
        assign_parked_names(&candidates, &taken, &claims)
    }

    #[test]
    fn assign_parked_names_cases() {
        for (label, folders, linked, proxies, expected) in ASSIGN_CASES {
            let got = assign(folders, linked, proxies, Vec::new(), &[]);
            let got: Vec<Option<&str>> = got.iter().map(Option::as_deref).collect();
            assert_eq!(got, *expected, "{label}");
        }
    }

    #[test]
    fn assign_parked_names_respects_explicitly_added_domains() {
        let own_root = doc_root_for(0, "laravel.com");
        let own_root = own_root.as_str();
        let cases: &[(&str, Vec<AddedDomain<'_>>, Option<&str>)] = &[
            (
                "linked site added the name",
                vec![(None, "laravel")],
                Some("laravel-com"),
            ),
            (
                "another parked site added the name",
                vec![(Some("/elsewhere"), "laravel")],
                Some("laravel-com"),
            ),
            (
                "the folder's own delta added the name",
                vec![(Some(own_root), "laravel")],
                Some("laravel"),
            ),
            (
                "a subdomain does not block the apex",
                vec![(None, "api.laravel")],
                Some("laravel"),
            ),
        ];
        for (label, added, expected) in cases {
            let got = assign(&["laravel.com"], &[], &[], added.clone(), &[]);
            assert_eq!(got.first().unwrap().as_deref(), *expected, "{label}");
        }
    }

    #[test]
    fn taken_names_collects_exact_added_domains_only() {
        let mut cfg = yerd_config::Config::default();
        cfg.domains.linked.insert(
            "shop".to_owned(),
            yerd_config::DomainDelta {
                added: vec![
                    yerd_core::Domain::parse_subpart("laravel").unwrap(),
                    yerd_core::Domain::parse_subpart("*.wild").unwrap(),
                ],
                ..yerd_config::DomainDelta::default()
            },
        );
        cfg.domains.parked.insert(
            "/srv/blog".to_owned(),
            yerd_config::DomainDelta {
                added: vec![yerd_core::Domain::parse_subpart("acme").unwrap()],
                ..yerd_config::DomainDelta::default()
            },
        );
        let taken = TakenNames::from_config(&cfg);
        assert!(taken.added_by_other("laravel", "/srv/x"));
        assert!(taken.added_by_other("acme", "/srv/x"));
        assert!(!taken.added_by_other("acme", "/srv/blog"));
        assert!(!taken.added_by_other("wild", "/srv/x"));
        assert!(!taken.added.iter().any(|(_, d)| d.starts_with('*')));
    }

    fn rename(legacy: &str, new: &str, doc_root: &str) -> ParkedRename {
        ParkedRename {
            legacy: legacy.to_owned(),
            new: new.to_owned(),
            doc_root: doc_root.to_owned(),
        }
    }

    fn rekey_fixture() -> yerd_config::Config {
        let mut cfg = yerd_config::Config::default();
        cfg.favourites.sites.insert("laravel-com".to_owned());
        cfg.groups.order.push("Work".to_owned());
        cfg.groups
            .members
            .insert("laravel-com".to_owned(), "Work".to_owned());
        cfg.tunnel.sites.insert(
            "laravel-com.test".to_owned(),
            "laravel.example.com".to_owned(),
        );
        cfg.tunnel
            .passwords
            .insert("laravel-com".to_owned(), "secret".to_owned());
        cfg.services.instances.insert(
            "reverb:laravel-com".to_owned(),
            yerd_config::ServiceInstance {
                site: Some("laravel-com".to_owned()),
                enabled: false,
                ..yerd_config::ServiceInstance::default()
            },
        );
        cfg.domains.parked.insert(
            "/srv/laravel.com".to_owned(),
            yerd_config::DomainDelta {
                added: vec![yerd_core::Domain::apex("laravel-com")],
                suppressed: vec![yerd_core::Domain::apex("laravel-com")],
                primary: Some(yerd_core::Domain::apex("laravel-com")),
            },
        );
        cfg
    }

    #[test]
    fn rekey_parked_names_moves_name_keyed_settings() {
        let mut cfg = rekey_fixture();
        let renames = [rename("laravel-com", "laravel", "/srv/laravel.com")];

        assert!(rekey_parked_names(&mut cfg, &renames));

        assert!(cfg.favourites.sites.contains("laravel"));
        assert!(!cfg.favourites.sites.contains("laravel-com"));
        assert_eq!(
            cfg.groups.members.get("laravel").map(String::as_str),
            Some("Work")
        );
        assert!(!cfg.groups.members.contains_key("laravel-com"));
        assert_eq!(
            cfg.tunnel.sites.get("laravel").map(String::as_str),
            Some("laravel.example.com")
        );
        assert!(!cfg.tunnel.sites.contains_key("laravel-com.test"));
        assert_eq!(
            cfg.tunnel.passwords.get("laravel").map(String::as_str),
            Some("secret")
        );
        let reverb = cfg.services.instances.get("reverb:laravel").unwrap();
        assert_eq!(reverb.site.as_deref(), Some("laravel"));
        assert!(!cfg.services.instances.contains_key("reverb:laravel-com"));
        let delta = cfg.domains.parked.get("/srv/laravel.com").unwrap();
        assert_eq!(delta.added, vec![yerd_core::Domain::apex("laravel-com")]);
        assert_eq!(delta.suppressed, vec![yerd_core::Domain::apex("laravel")]);
        assert_eq!(delta.primary, Some(yerd_core::Domain::apex("laravel")));

        assert!(!rekey_parked_names(&mut cfg, &renames));
    }

    #[test]
    fn rekey_parked_names_never_overwrites_the_new_key() {
        let mut cfg = rekey_fixture();
        cfg.groups.order.push("Home".to_owned());
        cfg.groups
            .members
            .insert("laravel".to_owned(), "Home".to_owned());
        cfg.tunnel
            .passwords
            .insert("laravel".to_owned(), "kept".to_owned());
        cfg.services.instances.insert(
            "reverb:laravel".to_owned(),
            yerd_config::ServiceInstance {
                site: Some("laravel".to_owned()),
                enabled: true,
                ..yerd_config::ServiceInstance::default()
            },
        );
        let renames = [rename("laravel-com", "laravel", "/srv/laravel.com")];

        assert!(rekey_parked_names(&mut cfg, &renames));

        assert_eq!(
            cfg.groups.members.get("laravel").map(String::as_str),
            Some("Home")
        );
        assert_eq!(
            cfg.tunnel.passwords.get("laravel").map(String::as_str),
            Some("kept")
        );
        assert!(
            cfg.services
                .instances
                .get("reverb:laravel")
                .unwrap()
                .enabled
        );
        assert!(cfg.services.instances.contains_key("reverb:laravel-com"));
    }

    type ClaimCase = (
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
        &'static [(usize, &'static str)],
        &'static [Option<&'static str>],
    );

    const CLAIM_CASES: &[ClaimCase] = &[
        (
            "a claimed name beats a new plain folder, which is hidden",
            &["laravel", "laravel.com"],
            &[],
            &[(1, "laravel")],
            &[None, Some("laravel")],
        ),
        (
            "a new suffixed folder cannot take a claimed name",
            &["laravel.app", "laravel.com"],
            &[],
            &[(1, "laravel")],
            &[Some("laravel-app"), Some("laravel")],
        ),
        (
            "the contest only counts unclaimed folders",
            &["laravel.app", "laravel.com", "laravel.org"],
            &[],
            &[(1, "laravel")],
            &[Some("laravel-app"), Some("laravel"), Some("laravel-org")],
        ),
        (
            "a claim yields to a linked site",
            &["laravel.com"],
            &["laravel"],
            &[(0, "laravel")],
            &[Some("laravel-com")],
        ),
        (
            "a stale claim for another name is ignored",
            &["laravel.com"],
            &[],
            &[(0, "other")],
            &[Some("laravel")],
        ),
        (
            "a claimant's legacy slug stays reserved against duplicates",
            &["laravel-com", "laravel.com"],
            &[],
            &[(1, "laravel")],
            &[None, Some("laravel")],
        ),
    ];

    #[test]
    fn assign_parked_names_honours_claims() {
        for (label, folders, linked, claimed, expected) in CLAIM_CASES {
            let got = assign(folders, linked, &[], Vec::new(), claimed);
            let got: Vec<Option<&str>> = got.iter().map(Option::as_deref).collect();
            assert_eq!(got, *expected, "{label}");
        }
    }

    fn claim(name: &str, url_synced: bool) -> yerd_config::ParkedClaim {
        yerd_config::ParkedClaim {
            name: name.to_owned(),
            url_synced,
        }
    }

    fn parked_router(sites: &[(&str, &str)]) -> SiteRouter {
        let mut router = SiteRouter::new(yerd_core::RouterConfig::with_tld(
            yerd_core::Tld::new("test").unwrap(),
        ));
        for (name, doc_root) in sites {
            router
                .insert(Site::parked(name, *doc_root, PhpVersion::new(8, 3)).unwrap())
                .unwrap();
        }
        router
    }

    #[test]
    fn reconcile_parked_claims_adds_and_prunes() {
        let router = parked_router(&[
            ("laravel", "/srv/laravel.com"),
            ("acme", "/srv/acme.io"),
            ("blog", "/srv/blog"),
            ("shop-com", "/srv/shop.com"),
        ]);
        let mut cfg = yerd_config::Config::default();
        cfg.parked
            .claims
            .insert("/srv/gone.com".to_owned(), claim("gone", true));
        cfg.parked
            .claims
            .insert("/srv/shop.com".to_owned(), claim("shop", true));
        cfg.parked
            .claims
            .insert("/srv/acme.io".to_owned(), claim("acme", false));
        cfg.parked.claims.insert(
            "/nonexistent-yerd-test/unmounted.com".to_owned(),
            claim("unmounted", false),
        );
        let pending: HashSet<&str> = ["/srv/laravel.com"].into_iter().collect();

        assert!(reconcile_parked_claims(&mut cfg, &router, &pending));

        let got: Vec<(&str, &str, bool)> = cfg
            .parked
            .claims
            .iter()
            .map(|(root, c)| (root.as_str(), c.name.as_str(), c.url_synced))
            .collect();
        assert_eq!(
            got,
            [
                ("/nonexistent-yerd-test/unmounted.com", "unmounted", false),
                ("/srv/acme.io", "acme", false),
                ("/srv/laravel.com", "laravel", false),
            ]
        );
        assert!(!reconcile_parked_claims(&mut cfg, &router, &HashSet::new()));
    }

    struct AddedCase {
        label: &'static str,
        served: &'static [(&'static str, &'static str)],
        roots: &'static [&'static str],
        folders: &'static [&'static str],
        pending: &'static [&'static str],
        existing: &'static [(&'static str, u64)],
        expected: &'static [(&'static str, u64)],
        expected_pending: &'static [&'static str],
        changed: bool,
    }

    const NOW: u64 = 1_764_100_000;

    const ADDED_CASES: &[AddedCase] = &[
        AddedCase {
            label: "the upgrade baseline records existing sites undated",
            served: &[("a", "/srv/a")],
            roots: &["/srv"],
            folders: &[],
            pending: &["/srv"],
            existing: &[],
            expected: &[("/srv/a", 0)],
            expected_pending: &[],
            changed: true,
        },
        AddedCase {
            label: "a root missing during the upgrade stays pending",
            served: &[("a", "/srv/a")],
            roots: &["/mnt", "/srv"],
            folders: &[],
            pending: &["/mnt", "/srv"],
            existing: &[],
            expected: &[("/srv/a", 0)],
            expected_pending: &["/mnt"],
            changed: true,
        },
        AddedCase {
            label: "a pending root that returns is baselined, not stamped",
            served: &[("b", "/mnt/b"), ("a", "/srv/a")],
            roots: &["/mnt", "/srv"],
            folders: &[],
            pending: &["/mnt"],
            existing: &[("/srv/a", 0)],
            expected: &[("/mnt/b", 0), ("/srv/a", 0)],
            expected_pending: &[],
            changed: true,
        },
        AddedCase {
            label: "an unparked root leaves the baseline",
            served: &[],
            roots: &[],
            folders: &[],
            pending: &["/old"],
            existing: &[],
            expected: &[],
            expected_pending: &[],
            changed: true,
        },
        AddedCase {
            label: "a new site after the baseline is stamped now",
            served: &[("a", "/srv/a")],
            roots: &["/srv"],
            folders: &[],
            pending: &[],
            existing: &[],
            expected: &[("/srv/a", NOW)],
            expected_pending: &[],
            changed: true,
        },
        AddedCase {
            label: "an existing entry is not overwritten",
            served: &[("a", "/srv/a")],
            roots: &["/srv"],
            folders: &[],
            pending: &[],
            existing: &[("/srv/a", 5)],
            expected: &[("/srv/a", 5)],
            expected_pending: &[],
            changed: false,
        },
        AddedCase {
            label: "a vanished site is pruned",
            served: &[("a", "/srv/a")],
            roots: &["/srv"],
            folders: &[],
            pending: &[],
            existing: &[("/srv/a", 5), ("/srv/gone", 5)],
            expected: &[("/srv/a", 5)],
            expected_pending: &[],
            changed: true,
        },
        AddedCase {
            label: "a hidden folder that still exists is kept",
            served: &[("a", "/srv/a")],
            roots: &["/srv"],
            folders: &["/srv/blog"],
            pending: &[],
            existing: &[("/srv/a", 5), ("/srv/blog", 7)],
            expected: &[("/srv/a", 5), ("/srv/blog", 7)],
            expected_pending: &[],
            changed: false,
        },
        AddedCase {
            label: "a root serving no sites keeps its entries",
            served: &[],
            roots: &["/mnt"],
            folders: &[],
            pending: &[],
            existing: &[("/mnt/a", 5)],
            expected: &[("/mnt/a", 5)],
            expected_pending: &[],
            changed: false,
        },
        AddedCase {
            label: "an unparked root's entries are pruned",
            served: &[],
            roots: &[],
            folders: &["/mnt/a"],
            pending: &[],
            existing: &[("/mnt/a", 5)],
            expected: &[],
            expected_pending: &[],
            changed: true,
        },
    ];

    #[test]
    fn reconcile_parked_added_cases() {
        for case in ADDED_CASES {
            let router = parked_router(case.served);
            let mut cfg = yerd_config::Config::default();
            cfg.parked
                .paths
                .extend(case.roots.iter().map(|r| (*r).to_owned()));
            cfg.added
                .baseline_pending
                .extend(case.pending.iter().map(|r| (*r).to_owned()));
            cfg.added.parked.extend(
                case.existing
                    .iter()
                    .map(|(root, t)| ((*root).to_owned(), *t)),
            );

            let changed =
                reconcile_parked_added(&mut cfg, &router, NOW, |p| case.folders.contains(&p));

            let got: Vec<(&str, u64)> = cfg
                .added
                .parked
                .iter()
                .map(|(root, t)| (root.as_str(), *t))
                .collect();
            assert_eq!(got, case.expected, "{}", case.label);
            let pending: Vec<&str> = cfg
                .added
                .baseline_pending
                .iter()
                .map(String::as_str)
                .collect();
            assert_eq!(pending, case.expected_pending, "{}", case.label);
            assert_eq!(changed, case.changed, "{}", case.label);
        }
    }

    #[test]
    fn parked_renames_skip_claimed_and_live_legacy_names() {
        let router = parked_router(&[
            ("laravel", "/srv/laravel.com"),
            ("acme", "/srv/acme.io"),
            ("shop", "/srv/shop.com"),
            ("shop-com", "/srv/other/shop-com"),
        ]);
        let mut claims = BTreeMap::new();
        claims.insert("/srv/acme.io".to_owned(), claim("acme", true));
        let renames = parked_renames(&router, &claims);
        assert_eq!(
            renames,
            [rename("laravel-com", "laravel", "/srv/laravel.com")]
        );
    }

    #[test]
    fn live_parked_renames_need_the_folder_served_under_its_legacy_name_before() {
        let router = parked_router(&[
            ("laravel", "/srv/laravel.com"),
            ("acme", "/srv/acme.io"),
            ("shop", "/srv/shop.com"),
        ]);
        let previous = parked_router(&[
            ("laravel-com", "/srv/laravel.com"),
            ("acme-io", "/srv/other/acme-io"),
        ]);
        assert_eq!(
            live_parked_renames(&previous, &router, &BTreeMap::new()),
            [rename("laravel-com", "laravel", "/srv/laravel.com")]
        );
        assert_eq!(
            renamed_sites(&[rename("laravel-com", "laravel", "/srv/laravel.com")]),
            [RenamedSite {
                name: "laravel".to_owned(),
                doc_root: "/srv/laravel.com".to_owned(),
            }]
        );
    }

    #[tokio::test]
    async fn url_sync_stays_pending_while_the_web_ports_are_unbound() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = crate::test_support::state_in(tmp.path());
        state.web_unbound = Some(yerd_ipc::UnboundWeb {
            http: 8080,
            https: 8443,
        });
        let root = tmp.path().join("laravel.com");
        std::fs::create_dir_all(&root).unwrap();
        let key = root.to_string_lossy().into_owned();
        state
            .config
            .lock()
            .await
            .parked
            .claims
            .insert(key.clone(), claim("laravel", false));
        *state.router.write().await = parked_router(&[("laravel", key.as_str())]);
        let pending = pending_url_syncs(&*state.config.lock().await);

        sync_renamed_site_urls(&state, &pending).await;

        assert!(!state.config.lock().await.parked.claims[&key].url_synced);
    }

    #[test]
    fn pending_url_syncs_lists_unsynced_claims() {
        let mut cfg = yerd_config::Config::default();
        cfg.parked
            .claims
            .insert("/srv/laravel.com".to_owned(), claim("laravel", false));
        cfg.parked
            .claims
            .insert("/srv/acme.io".to_owned(), claim("acme", true));
        assert_eq!(
            pending_url_syncs(&cfg),
            [RenamedSite {
                name: "laravel".to_owned(),
                doc_root: "/srv/laravel.com".to_owned(),
            }]
        );
    }

    #[tokio::test]
    async fn url_sync_stays_pending_unless_the_router_serves_that_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        state
            .config
            .lock()
            .await
            .parked
            .claims
            .insert("/srv/laravel.com".to_owned(), claim("laravel", false));
        *state.router.write().await = parked_router(&[("laravel", "/srv/elsewhere.com")]);
        let pending = pending_url_syncs(&*state.config.lock().await);

        sync_renamed_site_urls(&state, &pending).await;

        let cfg = state.config.lock().await;
        assert!(
            !cfg.parked
                .claims
                .get("/srv/laravel.com")
                .unwrap()
                .url_synced
        );
    }

    #[tokio::test]
    async fn url_sync_marks_a_plain_site_done() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        let root = tmp.path().join("laravel.com");
        std::fs::create_dir_all(&root).unwrap();
        let key = root.to_string_lossy().into_owned();
        state
            .config
            .lock()
            .await
            .parked
            .claims
            .insert(key.clone(), claim("laravel", false));
        *state.router.write().await = parked_router(&[("laravel", key.as_str())]);
        let pending = pending_url_syncs(&*state.config.lock().await);

        sync_renamed_site_urls(&state, &pending).await;

        assert!(
            state
                .config
                .lock()
                .await
                .parked
                .claims
                .get(&key)
                .unwrap()
                .url_synced
        );
    }

    #[tokio::test]
    async fn url_sync_does_nothing_after_shutdown() {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        let root = tmp.path().join("laravel.com");
        std::fs::create_dir_all(&root).unwrap();
        let key = root.to_string_lossy().into_owned();
        state
            .config
            .lock()
            .await
            .parked
            .claims
            .insert(key.clone(), claim("laravel", false));
        *state.router.write().await = parked_router(&[("laravel", key.as_str())]);
        let pending = pending_url_syncs(&*state.config.lock().await);
        state.shutdown_tx.send_replace(true);

        sync_renamed_site_urls(&state, &pending).await;

        assert!(
            !state
                .config
                .lock()
                .await
                .parked
                .claims
                .get(&key)
                .unwrap()
                .url_synced
        );
    }

    #[test]
    fn rekey_parked_names_without_renames_is_a_no_op() {
        let mut cfg = rekey_fixture();
        let before = cfg.clone();
        assert!(!rekey_parked_names(&mut cfg, &[]));
        assert_eq!(cfg, before);
    }

    #[test]
    fn load_or_default_config_uses_defaults_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("absent").join("yerd.toml");
        let cfg = load_or_default_config(&path).unwrap();
        assert_eq!(cfg.tld, yerd_config::Config::default().tld);
    }

    #[test]
    fn load_or_default_config_loads_existing_then_rejects_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("yerd.toml");
        yerd_config::Config::default().save(&path).unwrap();
        assert!(load_or_default_config(&path).is_ok());
        std::fs::write(&path, b"this is = not valid = toml {{{").unwrap();
        assert!(load_or_default_config(&path).is_err());
    }

    #[test]
    fn load_or_generate_ca_generates_then_reloads() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        std::fs::create_dir_all(&dirs.data).unwrap();
        let ca = load_or_generate_ca(&dirs).unwrap();
        let fp1 = ca.fingerprint_sha256();
        assert!(dirs.data.join("ca.cert.pem").is_file());
        assert!(dirs.data.join("ca.key.pem").is_file());
        let ca2 = load_or_generate_ca(&dirs).unwrap();
        assert_eq!(ca2.fingerprint_sha256(), fp1);
    }

    #[tokio::test]
    async fn into_tokio_listener_converts_a_bound_socket() {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let local = std_listener.local_addr().unwrap();
        let tokio_listener = into_tokio_listener(std_listener).unwrap();
        assert_eq!(tokio_listener.local_addr().unwrap(), local);
    }

    #[test]
    fn ca_validity_spans_the_past_into_the_future() {
        let v = ca_validity().unwrap();
        let now = time::OffsetDateTime::now_utc();
        assert!(v.not_before() < now);
        assert!(v.not_after() > now);
    }

    fn test_ca() -> CertAuthority {
        CertAuthority::generate(yerd_core::CA_COMMON_NAME, ca_validity().unwrap()).unwrap()
    }

    const FAKE_ROOT_PEM: &str =
        "-----BEGIN CERTIFICATE-----\nMIIFAKEROOTBLOCK\n-----END CERTIFICATE-----\n";

    #[test]
    fn build_php_ca_bundle_with_roots_returns_path_and_writes_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let ca = test_ca();
        let out = build_php_ca_bundle(&dirs, ca.cert_pem(), Some(FAKE_ROOT_PEM));
        let path = dirs.data.join("cacert.pem");
        assert_eq!(out.as_deref(), Some(path.as_path()));
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("MIIFAKEROOTBLOCK"));
        assert!(written.contains(ca.cert_pem().trim()));
        assert!(written.matches("BEGIN CERTIFICATE").count() >= 2);
    }

    #[test]
    fn build_php_ca_bundle_without_roots_returns_none_and_does_not_write() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let ca = test_ca();
        let out = build_php_ca_bundle(&dirs, ca.cert_pem(), None);
        assert!(out.is_none());
        assert!(
            !dirs.data.join("cacert.pem").exists(),
            "must not write a rootless bundle"
        );
    }

    #[test]
    fn build_php_ca_bundle_with_rootless_content_returns_none_and_does_not_write() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let ca = test_ca();
        let out = build_php_ca_bundle(&dirs, ca.cert_pem(), Some("garbage, no cert block"));
        assert!(out.is_none());
        assert!(!dirs.data.join("cacert.pem").exists());
    }

    #[test]
    fn build_php_ca_bundle_no_roots_reuses_existing_good_bundle_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let ca = test_ca();
        let good = build_php_ca_bundle(&dirs, ca.cert_pem(), Some(FAKE_ROOT_PEM)).unwrap();
        let before = std::fs::read_to_string(&good).unwrap();
        let reused = build_php_ca_bundle(&dirs, ca.cert_pem(), None);
        assert_eq!(reused.as_deref(), Some(good.as_path()));
        assert_eq!(std::fs::read_to_string(&good).unwrap(), before);
    }

    #[test]
    fn build_php_ca_bundle_no_roots_does_not_reuse_ca_only_or_stale_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let ca = test_ca();
        std::fs::create_dir_all(&dirs.data).unwrap();
        let path = dirs.data.join("cacert.pem");

        std::fs::write(&path, format!("{}\n", ca.cert_pem().trim())).unwrap();
        assert!(
            build_php_ca_bundle(&dirs, ca.cert_pem(), None).is_none(),
            "a CA-only bundle (one block, no public roots) must not be reused"
        );

        let other = test_ca();
        std::fs::write(
            &path,
            format!("{}\n{}\n", FAKE_ROOT_PEM.trim(), other.cert_pem().trim()),
        )
        .unwrap();
        assert!(
            build_php_ca_bundle(&dirs, ca.cert_pem(), None).is_none(),
            "a bundle missing the current CA must not be reused"
        );
    }
}
