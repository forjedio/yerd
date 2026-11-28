//! Test-only helpers shared across daemon unit tests.
//!
//! The signed-`php.json` fetch path (`php_install::fetch_verified_listing`)
//! verifies a **prehashed** minisign signature, so tests that exercise install /
//! update / available-versions need a validly-signed manifest. `minisign-verify`
//! only verifies, so we generate a throwaway keypair and sign at runtime with
//! the `minisign` crate (dev-dependency), handing the generated public key to
//! the code under test.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};

use yerd_core::{RouterConfig, SiteRouter, Tld};
use yerd_platform::PlatformDirs;

use crate::state::DaemonState;

/// A [`yerd_php::CommandRunner`] that never spawns anything and reports a
/// module list without FFI, so daemon unit tests never exec a fake install's
/// `bin/php`.
pub struct NoFfiRunner;

#[async_trait::async_trait]
impl yerd_php::CommandRunner for NoFfiRunner {
    async fn run(&self, _cmd: std::process::Command) -> std::io::Result<yerd_php::ProbeOutput> {
        Ok(yerd_php::ProbeOutput {
            status_ok: true,
            stdout: "[PHP Modules]\nCore\n".to_owned(),
            stderr: String::new(),
        })
    }
}

/// A [`yerd_php::Downloader`] serving fixed bodies by exact URL. Any other URL
/// fails like an offline network.
#[derive(Default)]
pub struct UrlDl(pub std::collections::HashMap<String, Vec<u8>>);

impl UrlDl {
    /// A downloader serving `body` at `url`, in addition to what it already serves.
    #[must_use]
    pub fn with(mut self, url: &str, body: &str) -> Self {
        self.0.insert(url.to_owned(), body.as_bytes().to_vec());
        self
    }
}

#[async_trait::async_trait]
impl yerd_php::Downloader for UrlDl {
    async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
        self.0
            .get(url)
            .cloned()
            .ok_or_else(|| yerd_php::DownloadError::Transport {
                url: url.to_owned(),
                reason: "offline".into(),
            })
    }
}

/// A tempdir-rooted [`PlatformDirs`] for a daemon unit test - five short,
/// disjoint subdirectories, none of which need to exist on disk beforehand.
#[must_use]
pub fn dirs_in(tmp: &Path) -> PlatformDirs {
    PlatformDirs {
        config: tmp.join("c"),
        data: tmp.join("d"),
        state: tmp.join("s"),
        cache: tmp.join("ca"),
        runtime: tmp.join("r"),
    }
}

/// Lay down a fake bundled PHP-FPM binary so `discover_bundled` reports
/// `version` as installed.
pub fn fake_php(dirs: &PlatformDirs, version: &str) {
    let sbin = dirs
        .data
        .join("php")
        .join(format!("php-{version}"))
        .join("sbin");
    std::fs::create_dir_all(&sbin).unwrap();
    std::fs::write(sbin.join("php-fpm"), b"x").unwrap();
}

/// A fully-populated [`DaemonState`] for a daemon unit test: an empty router,
/// a fresh `PhpManager` over `dirs_in(tmp)`, and every other field at its
/// harmless default (no mail listening, no tunnels, an empty `wordpress_sites`
/// cache, `wordpress_login_prepend_script: None`). A test that needs a
/// non-default value mutates the returned `DaemonState` directly - every
/// field is `pub` within the crate - rather than this function growing an
/// ever-longer parameter list for every caller's one differing field.
#[must_use]
pub fn state_in(tmp: &Path) -> DaemonState {
    let dirs = dirs_in(tmp);
    let router = SiteRouter::new(RouterConfig::with_tld(Tld::new("test").unwrap()));
    let ca_path = dirs.data.join("ca.cert.pem");
    let insights_root = crate::insights::Insights::store_root(&dirs.state);
    let php_manager = Arc::new(Mutex::new(yerd_php::PhpManager::new(
        yerd_php::TokioProcessSpawner,
        yerd_php::SystemClock,
        yerd_php::io::FastCgiProbe,
        dirs.clone(),
        yerd_platform::ActivePortBinder::new(),
        std::process::id(),
        std::collections::BTreeMap::new(),
    )));
    DaemonState {
        config: Mutex::new(yerd_config::Config::default()),
        router: Arc::new(RwLock::new(router)),
        config_path: dirs.config.join("yerd.toml"),
        dirs: dirs.clone(),
        dns_addr: "127.0.0.1:1053".parse().unwrap(),
        ca_path,
        ca_fingerprint: yerd_platform::CaFingerprint::new([0u8; 32]),
        php_ca_bundle: None,
        php_updates: RwLock::new(std::collections::HashMap::new()),
        node_updates: RwLock::new(std::collections::BTreeMap::new()),
        node_update_count: std::sync::atomic::AtomicU32::new(0),
        tool_updates: RwLock::new(std::collections::BTreeMap::new()),
        release_notes_cache: RwLock::new(None),
        contributors_cache: RwLock::new(None),
        yerd_update: RwLock::new(Vec::new()),
        update_snapshot: RwLock::new(None),
        php_manager,
        service_manager: Arc::new(Mutex::new(crate::services::new_manager(dirs))),
        mail_store: Arc::new(yerd_mail::Store::open(tmp.join("mail")).unwrap()),
        mail: crate::state::MailRuntime { listening: false },
        http: yerd_ipc::PortStatus {
            requested: 80,
            bound: 8080,
            fell_back: true,
        },
        https: yerd_ipc::PortStatus {
            requested: 443,
            bound: 8443,
            fell_back: true,
        },
        redirect_https_port: Arc::new(std::sync::atomic::AtomicU16::new(8443)),
        symlink_protection: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        web_unbound: None,
        dns_unbound: None,
        boot_id: 1,
        started_at: std::time::Instant::now(),
        shutdown_tx: tokio::sync::watch::channel(false).0,
        restart_requested: std::sync::atomic::AtomicBool::new(false),
        detect_cache: Arc::new(crate::detect_cache::DetectCache::new()),
        ffi_cache: Arc::new(crate::ffi_cache::FfiCache::new(Arc::new(NoFfiRunner))),
        libvips_probe: Arc::new(yerd_platform::FakeLibvipsProbe { found: None }),
        network: crate::network::NetworkRuntime::new(
            Arc::new(yerd_platform::FakeSystemProxySource::default()),
            false,
            crate::network::no_env,
            Arc::new(|| Box::pin(async { None })),
        ),
        watch_dirty: tokio::sync::Notify::new(),
        config_unsaved: std::sync::atomic::AtomicBool::new(false),
        dumps: Arc::new(crate::dump_server::DumpStore::new()),
        insights: Arc::new(crate::insights::Insights::new(
            yerd_insights::Store::new(insights_root),
            1,
            &yerd_config::Config::default(),
        )),
        shim_reconcile: Mutex::new(()),
        tunnel_manager: Arc::new(Mutex::new(crate::tunnel::new_manager())),
        cloudflared_resolution: RwLock::new(None),
        tunnel_origin_port: None,
        tunnel_routes: Arc::new(RwLock::new(std::collections::HashMap::new())),
        tunnel_routes_rebuild: Mutex::new(()),
        share_warnings: Mutex::new(std::collections::HashMap::new()),
        named_stop_epoch: std::sync::atomic::AtomicU64::new(0),
        tunnel_metrics_addr: Arc::new(crate::tunnel::pick_metrics_addr),
        tool_mutate: Mutex::new(()),
        tunnel_mutate: Mutex::new(()),
        zrok_mutate: Mutex::new(()),
        php_mutate: Mutex::new(()),
        php_settings_mutate: Mutex::new(()),
        jobs: crate::jobs::JobRegistry::default(),
        reserved_names: Mutex::new(std::collections::HashSet::new()),
        project_file_mutate: Mutex::new(()),
        url_sync: Mutex::new(()),
        wordpress_versions: RwLock::new(None),
        wordpress_login_tokens: Arc::new(crate::wordpress_login::LoginTokenRegistry::new()),
        wordpress_login_prepend_script: None,
        wordpress_sites: Arc::new(RwLock::new(std::collections::HashMap::new())),
        laravel_sites: Arc::new(RwLock::new(std::collections::HashMap::new())),
        framework_sites: Arc::new(RwLock::new(std::collections::HashMap::new())),
        favourite_sites: RwLock::new(std::collections::BTreeSet::new()),
        site_added: RwLock::new(yerd_config::AddedSection::default()),
        lan_ip: None,
        lan_setup_bound: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        remote_setup_code: tokio::sync::Mutex::new(None),
        lan_setup_script_sha256: tokio::sync::Mutex::new(None),
    }
}

/// A `php.json` body plus its detached minisign signature and the public key it
/// was signed with. Feed `public_key` to `fetch_verified_listing`, and serve
/// `manifest` / `minisig` from a fake `Downloader`.
pub struct SignedManifest {
    /// Base64 public-key line accepted by `yerd_update::verify_minisign`.
    pub public_key: String,
    /// The `php.json` body.
    pub manifest: String,
    /// The detached `php.json.minisig` file contents.
    pub minisig: String,
}

/// Sign `manifest` with a freshly generated keypair (prehashed, as yerd
/// requires) and return it with its signature and public key. Panics on any
/// crypto error - test-only.
///
/// Before returning, the signature is re-checked with the **production**
/// `verify_minisign` so that if the signing crate ever stops producing a
/// prehashed/non-legacy signature this fails loudly here, rather than turning
/// every downstream test red for an opaque reason.
#[must_use]
pub fn sign_manifest(manifest: &str) -> SignedManifest {
    let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let sig_box = minisign::sign(
        Some(&kp.pk),
        &kp.sk,
        std::io::Cursor::new(manifest.as_bytes()),
        Some("test manifest"),
        Some("yerd test"),
    )
    .unwrap();
    let out = SignedManifest {
        public_key: kp.pk.to_base64(),
        manifest: manifest.to_owned(),
        minisig: sig_box.into_string(),
    };
    yerd_update::verify_minisign(&out.public_key, &out.minisig, out.manifest.as_bytes())
        .expect("freshly signed manifest must verify with the production verifier");
    out
}

/// Sign two manifest bodies with a **single** freshly generated keypair, so both
/// share one public key - mirroring production, where `php.json` and
/// `php-legacy.json` are signed with the same embedded key. Returns the two
/// [`SignedManifest`]s (their `public_key` fields are identical).
#[must_use]
pub fn sign_manifest_pair(first: &str, second: &str) -> (SignedManifest, SignedManifest) {
    let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let public_key = kp.pk.to_base64();
    let sign_one = |body: &str| {
        let sig_box = minisign::sign(
            Some(&kp.pk),
            &kp.sk,
            std::io::Cursor::new(body.as_bytes()),
            Some("test manifest"),
            Some("yerd test"),
        )
        .unwrap();
        let out = SignedManifest {
            public_key: public_key.clone(),
            manifest: body.to_owned(),
            minisig: sig_box.into_string(),
        };
        yerd_update::verify_minisign(&out.public_key, &out.minisig, out.manifest.as_bytes())
            .expect("freshly signed manifest must verify with the production verifier");
        out
    };
    (sign_one(first), sign_one(second))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_production_verifier() {
        let s = sign_manifest(r#"{"schema":1,"builds":[]}"#);
        assert!(
            yerd_update::verify_minisign(&s.public_key, &s.minisig, s.manifest.as_bytes()).is_ok()
        );
        assert!(yerd_update::verify_minisign(&s.public_key, &s.minisig, b"tampered").is_err());
    }
}
