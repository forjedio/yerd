//! Pure resolution of prebuilt service download artifacts from **yerd's own**
//! hosted listing.
//!
//! Unlike PHP (which consumes the upstream `static-php-cli` distribution), there
//! is no single multi-platform distribution for databases, so yerd builds and
//! hosts its own (the `xtask services-build` matrix) and publishes a directory
//! listing in a uniform shape. The daemon fetches the listing and hands the body
//! to [`resolve_from_listing`] / [`available_versions`] (both pure).
//!
//! Artifact naming: `<service-id>-<version>-<os>-<arch>.tar.gz`
//! (e.g. `redis-8-linux-x86_64.tar.gz`). Integrity rests on HTTPS to the host.

use crate::error::ServiceError;
use crate::service::Service;
use crate::version::ServiceVersion;

/// Base URL of yerd's hosted service-binary distribution.
///
/// Hosted on GitHub Releases of the **separate** `forjedio/yerd-services` build
/// project (see `@docs/yerd-services-build-repo.md`): a single rolling `services`
/// release holds every `<service>-<version>-<os>-<arch>.tar.gz` asset plus the
/// generated `index.html` listing. Asset URLs 302-redirect to the blob; the
/// daemon's downloader follows redirects. This crate is a pure *consumer* — the
/// producer lives entirely in `forjedio/yerd-services`.
pub const SERVICES_BASE_URL: &str =
    "https://github.com/forjedio/yerd-services/releases/download/services";

/// Target operating system for a prebuilt artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Linux.
    Linux,
    /// macOS.
    Macos,
}

impl Os {
    /// The token used in artifact filenames.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Macos => "macos",
        }
    }
}

/// Target CPU architecture for a prebuilt artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// 64-bit x86.
    X86_64,
    /// 64-bit ARM.
    Aarch64,
}

impl Arch {
    /// The token used in artifact filenames.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

/// A resolved download plan for one service + version + platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// The service.
    pub service: Service,
    /// The resolved version.
    pub version: ServiceVersion,
    /// URL of the `.tar.gz` archive.
    pub url: String,
}

/// The artifact filename for a `(service, version, os, arch)`.
#[must_use]
pub fn artifact_filename(service: Service, version: &ServiceVersion, os: Os, arch: Arch) -> String {
    format!(
        "{}-{}-{}-{}.tar.gz",
        service.id(),
        version.as_str(),
        os.as_str(),
        arch.as_str()
    )
}

/// The full artifact URL for a `(service, version, os, arch)`.
#[must_use]
pub fn artifact_url(service: Service, version: &ServiceVersion, os: Os, arch: Arch) -> String {
    format!(
        "{SERVICES_BASE_URL}/{}",
        artifact_filename(service, version, os, arch)
    )
}

/// URL of the distribution's listing. We publish a generated `index.html` as a
/// release asset alongside the tarballs (GitHub Releases has no directory
/// autoindex), so the listing lives at `<base>/index.html`.
#[must_use]
pub fn listing_url() -> String {
    format!("{SERVICES_BASE_URL}/index.html")
}

/// Resolve a requested `(service, version)` + platform to an [`Artifact`] by
/// confirming the exact artifact is present in `listing`. Errors with
/// [`ServiceError::VersionUnavailable`] when no matching build is published.
pub fn resolve_from_listing(
    listing: &str,
    service: Service,
    version: &ServiceVersion,
    os: Os,
    arch: Arch,
) -> Result<Artifact, ServiceError> {
    let filename = artifact_filename(service, version, os, arch);
    if listing.contains(&filename) {
        Ok(Artifact {
            service,
            version: version.clone(),
            url: artifact_url(service, version, os, arch),
        })
    } else {
        Err(ServiceError::VersionUnavailable {
            service,
            version: version.clone(),
        })
    }
}

/// Every version of `service` published for `(os, arch)` in `listing`, ascending.
///
/// Scans for filenames `<id>-<version>-<os>-<arch>.tar.gz`, anchoring on the
/// `<id>-` prefix and the `-<os>-<arch>.tar.gz` suffix so the middle (which may
/// itself contain `-`) is the version, unambiguously.
#[must_use]
pub fn available_versions(
    listing: &str,
    service: Service,
    os: Os,
    arch: Arch,
) -> Vec<ServiceVersion> {
    let prefix = format!("{}-", service.id());
    let suffix = format!("-{}-{}.tar.gz", os.as_str(), arch.as_str());

    let mut out: Vec<ServiceVersion> = Vec::new();
    // Tokens are whitespace/quote/angle-bracket delimited in an autoindex page.
    for token in listing.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>')) {
        let Some(rest) = token.strip_prefix(&prefix) else {
            continue;
        };
        let Some(mid) = rest.strip_suffix(&suffix) else {
            continue;
        };
        if let Ok(v) = mid.parse::<ServiceVersion>() {
            out.push(v);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Detect the running platform, erroring on anything yerd can't install for
/// (Windows, 32-bit). Call this **before** any download.
pub fn current_os_arch() -> Result<(Os, Arch), ServiceError> {
    let os = match std::env::consts::OS {
        "linux" => Os::Linux,
        "macos" => Os::Macos,
        other => {
            return Err(ServiceError::UnsupportedPlatform {
                detail: format!("no prebuilt services for OS {other:?}"),
            })
        }
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => Arch::X86_64,
        "aarch64" => Arch::Aarch64,
        other => {
            return Err(ServiceError::UnsupportedPlatform {
                detail: format!("no prebuilt services for architecture {other:?}"),
            })
        }
    };
    Ok((os, arch))
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
    use std::str::FromStr;

    const LISTING: &str = r#"
        <a href="redis-7-linux-x86_64.tar.gz">redis-7-linux-x86_64.tar.gz</a>
        <a href="redis-8-linux-x86_64.tar.gz">redis-8-linux-x86_64.tar.gz</a>
        <a href="redis-8-macos-aarch64.tar.gz">redis-8-macos-aarch64.tar.gz</a>
        <a href="postgres-16-linux-x86_64.tar.gz">postgres-16-linux-x86_64.tar.gz</a>
        <a href="mysql-8.4-linux-x86_64.tar.gz">mysql-8.4-linux-x86_64.tar.gz</a>
    "#;

    fn v(s: &str) -> ServiceVersion {
        ServiceVersion::from_str(s).unwrap()
    }

    #[test]
    fn resolve_present_artifact_builds_url() {
        let a = resolve_from_listing(LISTING, Service::Redis, &v("8"), Os::Linux, Arch::X86_64)
            .unwrap();
        assert_eq!(
            a.url,
            format!("{SERVICES_BASE_URL}/redis-8-linux-x86_64.tar.gz")
        );
        assert_eq!(a.service, Service::Redis);
    }

    #[test]
    fn resolve_missing_artifact_errors() {
        assert!(matches!(
            resolve_from_listing(LISTING, Service::Redis, &v("9"), Os::Linux, Arch::X86_64),
            Err(ServiceError::VersionUnavailable { .. })
        ));
        // Right version, wrong arch.
        assert!(matches!(
            resolve_from_listing(LISTING, Service::Redis, &v("8"), Os::Linux, Arch::Aarch64),
            Err(ServiceError::VersionUnavailable { .. })
        ));
    }

    #[test]
    fn available_versions_anchors_service_and_platform() {
        let got = available_versions(LISTING, Service::Redis, Os::Linux, Arch::X86_64);
        assert_eq!(got, vec![v("7"), v("8")]);

        // macos/aarch64 redis only has 8.
        let got = available_versions(LISTING, Service::Redis, Os::Macos, Arch::Aarch64);
        assert_eq!(got, vec![v("8")]);

        // mysql has a dotted version.
        let got = available_versions(LISTING, Service::MySql, Os::Linux, Arch::X86_64);
        assert_eq!(got, vec![v("8.4")]);

        // Postgres present only on linux/x86_64.
        assert!(
            available_versions(LISTING, Service::Postgres, Os::Macos, Arch::Aarch64).is_empty()
        );
    }

    #[test]
    fn available_versions_empty_listing() {
        assert!(available_versions("", Service::Redis, Os::Linux, Arch::X86_64).is_empty());
    }
}
