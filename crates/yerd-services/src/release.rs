//! Pure resolution of prebuilt service download artifacts from **yerd's own**
//! hosted listing.
//!
//! Unlike PHP (which consumes the upstream `static-php-cli` distribution), there
//! is no single multi-platform distribution for databases, so yerd builds and
//! hosts its own (the `forjedio/yerd-services` build matrix) and publishes a
//! machine-readable `services.json` listing. The daemon fetches the listing and
//! hands the body to [`resolve_from_listing`] / [`available_versions`] (both
//! pure).
//!
//! ## Listing format (`services.json`)
//!
//! ```json
//! {
//!   "schema": 1,
//!   "services": {
//!     "redis":    { "versions": [ { "version": "9.1.0", "platforms": ["linux-x86_64", "macos-aarch64"] } ] },
//!     "postgres": { "versions": [ { "version": "17.10", "platforms": ["linux-x86_64"] } ] }
//!   }
//! }
//! ```
//!
//! We parse the declared `(version, platforms)` per service rather than scraping
//! filenames: the listing is the source of truth for *what exists*, and the
//! `<service-id>-<version>-<os>-<arch>.tar.gz` filename convention is only used
//! to build the download URL once a build is confirmed present. The `schema`
//! field gates compatibility — an unknown schema is rejected rather than
//! misparsed. Integrity rests on HTTPS to the host.

use serde::Deserialize;

use crate::error::ServiceError;
use crate::service::Service;
use crate::version::ServiceVersion;

/// The `services.json` schema version this build understands. A producer-side
/// bump signals an incompatible format change (additive changes do not bump it).
pub const LISTING_SCHEMA: u32 = 1;

// ── listing wire shape (private; deserialised from `services.json`) ──────────

#[derive(Debug, Deserialize)]
struct Listing {
    schema: u32,
    #[serde(default)]
    services: std::collections::BTreeMap<String, ServiceEntry>,
}

#[derive(Debug, Deserialize)]
struct ServiceEntry {
    #[serde(default)]
    versions: Vec<VersionEntry>,
}

#[derive(Debug, Deserialize)]
struct VersionEntry {
    version: String,
    #[serde(default)]
    platforms: Vec<String>,
}

/// Parse + schema-check a `services.json` body.
fn parse_listing(listing: &str) -> Result<Listing, ServiceError> {
    let parsed: Listing =
        serde_json::from_str(listing).map_err(|e| ServiceError::ListingParse {
            detail: e.to_string(),
        })?;
    if parsed.schema != LISTING_SCHEMA {
        return Err(ServiceError::UnsupportedListingSchema {
            found: parsed.schema,
            supported: LISTING_SCHEMA,
        });
    }
    Ok(parsed)
}

/// Base URL of yerd's hosted service-binary distribution.
///
/// Hosted on GitHub Releases of the **separate** `forjedio/yerd-services` build
/// project (see `@docs/yerd-services-build-repo.md`): a single rolling `services`
/// release holds every `<service>-<version>-<os>-<arch>.tar.gz` asset plus the
/// generated `services.json` listing (a human-facing `index.html` is published
/// alongside but is not consumed here). Asset URLs 302-redirect to the blob; the
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

/// The `<os>-<arch>` platform token used in the listing's `platforms` arrays
/// and in artifact filenames (e.g. `linux-x86_64`).
#[must_use]
pub fn platform_token(os: Os, arch: Arch) -> String {
    format!("{}-{}", os.as_str(), arch.as_str())
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

/// URL of the distribution's machine-readable listing, `<base>/services.json`.
#[must_use]
pub fn listing_url() -> String {
    format!("{SERVICES_BASE_URL}/services.json")
}

/// Resolve a requested `(service, version)` + platform to an [`Artifact`] by
/// confirming the build is declared in `listing` for this platform.
///
/// Errors with [`ServiceError::VersionUnavailable`] when no matching build is
/// published, [`ServiceError::ListingParse`] if the body is malformed, or
/// [`ServiceError::UnsupportedListingSchema`] on an unknown schema.
pub fn resolve_from_listing(
    listing: &str,
    service: Service,
    version: &ServiceVersion,
    os: Os,
    arch: Arch,
) -> Result<Artifact, ServiceError> {
    let parsed = parse_listing(listing)?;
    let platform = platform_token(os, arch);

    let present = parsed
        .services
        .get(service.id())
        .into_iter()
        .flat_map(|entry| entry.versions.iter())
        .any(|v| v.version == version.as_str() && v.platforms.iter().any(|p| p == &platform));

    if present {
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
/// A malformed or unknown-schema listing yields an empty list (callers treat
/// "no versions" and "couldn't read the listing" the same for availability), so
/// this is infallible by design; use [`resolve_from_listing`] when you need the
/// parse error surfaced.
#[must_use]
pub fn available_versions(
    listing: &str,
    service: Service,
    os: Os,
    arch: Arch,
) -> Vec<ServiceVersion> {
    let Ok(parsed) = parse_listing(listing) else {
        return Vec::new();
    };
    let platform = platform_token(os, arch);

    let mut out: Vec<ServiceVersion> = parsed
        .services
        .get(service.id())
        .into_iter()
        .flat_map(|entry| entry.versions.iter())
        .filter(|v| v.platforms.iter().any(|p| p == &platform))
        .filter_map(|v| v.version.parse::<ServiceVersion>().ok())
        .collect();
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

    // Shaped like the real `services.json`, with extra rows to exercise
    // per-platform and multi-version filtering.
    const LISTING: &str = r#"{
      "schema": 1,
      "services": {
        "redis": { "versions": [
          { "version": "7.4.0", "platforms": ["linux-x86_64"] },
          { "version": "9.1.0", "platforms": ["linux-x86_64", "macos-aarch64"] }
        ] },
        "postgres": { "versions": [
          { "version": "17.10", "platforms": ["linux-x86_64"] }
        ] },
        "mysql": { "versions": [
          { "version": "8.4.9", "platforms": ["linux-x86_64", "macos-aarch64"] }
        ] }
      }
    }"#;

    fn v(s: &str) -> ServiceVersion {
        ServiceVersion::from_str(s).unwrap()
    }

    #[test]
    fn resolve_present_artifact_builds_url() {
        let a = resolve_from_listing(
            LISTING,
            Service::Redis,
            &v("9.1.0"),
            Os::Linux,
            Arch::X86_64,
        )
        .unwrap();
        assert_eq!(
            a.url,
            format!("{SERVICES_BASE_URL}/redis-9.1.0-linux-x86_64.tar.gz")
        );
        assert_eq!(a.service, Service::Redis);
    }

    #[test]
    fn resolve_missing_artifact_errors() {
        // Version not published.
        assert!(matches!(
            resolve_from_listing(
                LISTING,
                Service::Redis,
                &v("8.0.0"),
                Os::Linux,
                Arch::X86_64
            ),
            Err(ServiceError::VersionUnavailable { .. })
        ));
        // Right version, platform not listed (7.4.0 is linux-x86_64 only).
        assert!(matches!(
            resolve_from_listing(
                LISTING,
                Service::Redis,
                &v("7.4.0"),
                Os::Macos,
                Arch::Aarch64
            ),
            Err(ServiceError::VersionUnavailable { .. })
        ));
        // Service not in the listing at all.
        assert!(matches!(
            resolve_from_listing(
                LISTING,
                Service::MariaDb,
                &v("11.4"),
                Os::Linux,
                Arch::X86_64
            ),
            Err(ServiceError::VersionUnavailable { .. })
        ));
    }

    #[test]
    fn available_versions_filters_by_service_and_platform() {
        // linux/x86_64 redis has both versions, ascending.
        let got = available_versions(LISTING, Service::Redis, Os::Linux, Arch::X86_64);
        assert_eq!(got, vec![v("7.4.0"), v("9.1.0")]);

        // macos/aarch64 redis only has 9.1.0.
        let got = available_versions(LISTING, Service::Redis, Os::Macos, Arch::Aarch64);
        assert_eq!(got, vec![v("9.1.0")]);

        // mysql, dotted version, present on macos.
        let got = available_versions(LISTING, Service::MySql, Os::Macos, Arch::Aarch64);
        assert_eq!(got, vec![v("8.4.9")]);

        // Postgres present only on linux/x86_64.
        assert!(
            available_versions(LISTING, Service::Postgres, Os::Macos, Arch::Aarch64).is_empty()
        );
        // MariaDb absent from the listing entirely.
        assert!(available_versions(LISTING, Service::MariaDb, Os::Linux, Arch::X86_64).is_empty());
    }

    #[test]
    fn available_versions_empty_or_malformed_listing() {
        assert!(available_versions("", Service::Redis, Os::Linux, Arch::X86_64).is_empty());
        assert!(available_versions("not json", Service::Redis, Os::Linux, Arch::X86_64).is_empty());
    }

    #[test]
    fn resolve_reports_parse_and_schema_errors() {
        assert!(matches!(
            resolve_from_listing(
                "}{ bad",
                Service::Redis,
                &v("9.1.0"),
                Os::Linux,
                Arch::X86_64
            ),
            Err(ServiceError::ListingParse { .. })
        ));
        let future = r#"{ "schema": 2, "services": {} }"#;
        assert!(matches!(
            resolve_from_listing(future, Service::Redis, &v("9.1.0"), Os::Linux, Arch::X86_64),
            Err(ServiceError::UnsupportedListingSchema {
                found: 2,
                supported: 1
            })
        ));
    }

    #[test]
    fn parses_the_real_published_listing_shape() {
        // The exact bytes the producer ships (mirrors services.json at the
        // rolling `services` release). Guards drift in either direction.
        let real = r#"{"schema":1,"services":{"mysql":{"versions":[{"version":"8.4.9","platforms":["linux-aarch64","linux-x86_64","macos-aarch64"]}]},"postgres":{"versions":[{"version":"17.10","platforms":["linux-aarch64","linux-x86_64","macos-aarch64"]}]},"redis":{"versions":[{"version":"9.1.0","platforms":["linux-aarch64","linux-x86_64","macos-aarch64"]}]}}}"#;
        let a = resolve_from_listing(real, Service::Redis, &v("9.1.0"), Os::Macos, Arch::Aarch64)
            .unwrap();
        assert_eq!(
            a.url,
            format!("{SERVICES_BASE_URL}/redis-9.1.0-macos-aarch64.tar.gz")
        );
        assert_eq!(
            available_versions(real, Service::Postgres, Os::Linux, Arch::Aarch64),
            vec![v("17.10")]
        );
    }
}
