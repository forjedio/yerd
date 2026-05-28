//! Public schema types.
//!
//! The public types implement neither `Serialize` nor `Deserialize` directly.
//! Round-trip goes through crate-internal wire mirrors in [`crate::parse`]
//! and [`crate::serialize`]. This keeps the public surface free of an
//! accidental serde contract that downstream consumers might rely on.

use std::collections::BTreeSet;

use yerd_core::{PhpVersion, Site, Tld};

/// Top-level on-disk config.
///
/// `version` is private. All `Config` values produced by this build carry
/// `version == crate::CURRENT_VERSION`; there is no public accessor because
/// it would only ever return that constant. Callers wanting the on-disk
/// version should read [`crate::CURRENT_VERSION`] directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub(crate) version: u32,
    /// TLD served by Yerd's resolver. Default: `"test"`.
    pub tld: Tld,
    /// HTTP / HTTPS listen ports.
    pub ports: Ports,
    /// PHP defaults.
    pub php: PhpSection,
    /// Parked directories.
    pub parked: ParkedSection,
    /// Explicitly linked sites. Order is preserved on round-trip.
    pub linked: Vec<Site>,
    /// Optional services.
    pub services: ServicesSection,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: crate::CURRENT_VERSION,
            tld: Tld::default(),
            ports: Ports::default(),
            php: PhpSection::default(),
            parked: ParkedSection::default(),
            linked: Vec::new(),
            services: ServicesSection::default(),
        }
    }
}

impl Config {
    /// Parse a TOML document. Runs schema-version routing → wire-mirror
    /// deserialisation → `TryFrom<Wire>` (which invokes `yerd-core`'s
    /// per-field validators on `Tld`, `PhpVersion`, and `Site`, surfacing
    /// [`crate::ConfigError::Core`] on failure) → [`Self::validate`].
    pub fn from_toml(s: &str) -> Result<Self, crate::ConfigError> {
        crate::parse::parse_toml(s)
    }

    /// Serialise to TOML. Always writes `version = CURRENT_VERSION`.
    pub fn to_toml(&self) -> Result<String, crate::ConfigError> {
        crate::serialize::to_toml(self)
    }

    /// Validate cross-field invariants, plus per-field invariants that the
    /// `BTreeSet<String>` storage of `parked.paths` and `services.enabled`
    /// cannot enforce structurally (empty strings, unknown services).
    /// Per-field invariants on typed fields (TLD, `PhpVersion`, `Site`
    /// name) are enforced earlier, during `Wire` → `Config` conversion.
    pub fn validate(&self) -> Result<(), crate::ConfigError> {
        crate::parse::validate(self)
    }

    /// Thin I/O leaf — read + parse a TOML file at `path`.
    pub fn load(path: &std::path::Path) -> Result<Self, crate::ConfigError> {
        crate::io::load(path)
    }

    /// Thin I/O leaf — serialise + save atomically via write-temp-then-rename.
    ///
    /// `save` may create intermediate parent directories
    /// (`fs::create_dir_all`); they are not removed on a later failure. On
    /// Unix the destination ends up with mode 0600 (owner read/write only)
    /// inherited from the temp file — the daemon is the only intended
    /// writer.
    ///
    /// A parent-less path (e.g. `Path::new("config.toml")`) is treated as
    /// relative to the process's current working directory.
    pub fn save(&self, path: &std::path::Path) -> Result<(), crate::ConfigError> {
        crate::io::save(self, path)
    }
}

/// HTTP and HTTPS ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    /// HTTP listen port. Default: 80.
    pub http: u16,
    /// HTTPS listen port. Default: 443.
    pub https: u16,
}

impl Ports {
    /// IANA well-known pair, `80 / 443`. Default. Binding these may require
    /// elevation on macOS / Linux; Windows does not gate them.
    #[must_use]
    pub const fn well_known() -> Self {
        Self {
            http: 80,
            https: 443,
        }
    }

    /// Unprivileged fallback, `8080 / 8443`.
    #[must_use]
    pub const fn unprivileged() -> Self {
        Self {
            http: 8080,
            https: 8443,
        }
    }
}

impl Default for Ports {
    fn default() -> Self {
        Self::well_known()
    }
}

/// PHP defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhpSection {
    /// Default PHP version applied to new sites.
    pub default: PhpVersion,
}

impl Default for PhpSection {
    fn default() -> Self {
        Self {
            default: PhpVersion::new(8, 3),
        }
    }
}

/// Parked directories.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParkedSection {
    /// Set of parked directory paths, stored verbatim as UTF-8 strings.
    ///
    /// `String` (not `PathBuf`) is intentional: this crate does not own
    /// platform-specific path semantics, and `PathBuf::serialize` is lossy
    /// for non-UTF-8 paths on Windows. Callers convert to `PathBuf` at the
    /// point of use.
    ///
    /// Storage is byte-exact — the config layer does not canonicalise.
    /// `"/srv/foo"` and `"/srv/foo/"` are distinct entries. Callers wanting
    /// equality semantics must normalise before insertion.
    ///
    /// `BTreeSet` so the serialiser yields stable lexicographic order and
    /// duplicates are structurally impossible.
    pub paths: BTreeSet<String>,
}

/// Enabled services.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServicesSection {
    /// Service identifiers the daemon should auto-start.
    ///
    /// On-disk wire shape is a TOML array of strings (`enabled = ["mysql"]`),
    /// pinned in `tests/toml_byte_shape.rs`. Validated against
    /// `KNOWN_SERVICES` (private const in `parse.rs`).
    ///
    /// Stringly-typed in v0 because (a) when `yerd-services` lands the
    /// canonical typed `Service` enum will live there (downstream of this
    /// crate), and (b) a string allows forward-compatibility with
    /// experimental services without a `yerd-config` release.
    pub enabled: BTreeSet<String>,
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
    fn default_config_carries_current_version() {
        assert_eq!(Config::default().version, crate::CURRENT_VERSION);
    }

    #[test]
    fn default_ports_is_well_known() {
        assert_eq!(Ports::default(), Ports::well_known());
        assert_eq!(Ports::well_known().http, 80);
        assert_eq!(Ports::well_known().https, 443);
    }

    #[test]
    fn unprivileged_ports_match_documented_fallback() {
        assert_eq!(Ports::unprivileged().http, 8080);
        assert_eq!(Ports::unprivileged().https, 8443);
    }

    #[test]
    fn default_php_section_is_8_3() {
        assert_eq!(PhpSection::default().default, PhpVersion::new(8, 3));
    }

    #[test]
    fn default_parked_and_services_empty() {
        assert!(ParkedSection::default().paths.is_empty());
        assert!(ServicesSection::default().enabled.is_empty());
    }
}
