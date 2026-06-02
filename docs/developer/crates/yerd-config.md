# yerd-config

`yerd-config` owns the on-disk configuration for Yerd: a single schema-versioned
TOML file. It is responsible for exactly four verbs - **parse**, **validate**,
**serialise**, and **migrate** - plus two thin I/O leaves that read and write
that file atomically. Nothing more.

The crate is built around a strict purity boundary. Everything except
[`Config::load`](#config-load) and [`Config::save`](#config-save) is a pure
function over strings and in-memory values: no filesystem, no network, no
environment reads. That makes the parse / validate / serialise pipeline trivially
testable with string inputs and keeps the side-effecting surface down to two
functions.

For the user-facing field reference (what each key means, defaults, examples),
see [Configuration Reference](../../reference/configuration). This page is the
contributor-facing tour of the source.

::: info Crate boundaries
`yerd-config` does **not** decide *where* the config lives, and it does **not**
scan the filesystem for sites. Path discovery is the caller's job (the daemon);
site scanning lives elsewhere. See [What it must not do](#what-it-must-not-do).
:::

## Dependencies

From `Cargo.toml`:

```toml
[dependencies]
yerd-core = { path = "../yerd-core" }
serde     = { workspace = true }
toml      = { workspace = true }
thiserror = { workspace = true }
tempfile  = { workspace = true }
```

- `yerd-core` supplies the validated domain types (`Tld`, `PhpVersion`, `Site`,
  `SiteKind`) and the `php_settings` validator. Per-field invariants are
  enforced by `yerd-core`, not re-implemented here.
- `toml` + `serde` drive (de)serialisation through crate-internal wire mirrors.
- `tempfile` provides `NamedTempFile` for the atomic write-temp-then-rename save.
- `thiserror` derives the [`ConfigError`](#errors) enum.

The crate sets `#![forbid(unsafe_code)]`.

## Module map

```
src/
  lib.rs        Re-exports + CURRENT_VERSION; the purity-boundary doc.
  schema.rs     Public types (Config, Ports, PhpSection, ParkedSection,
                SiteOverride, ServicesSection) + Config's public methods.
  parse.rs      Wire mirrors, TOML deserialisation, TryFrom<Wire>, validate().
  serialize.rs  Borrowed wire mirrors + to_toml().
  migrate.rs    Schema-version reading and forward-migration step walker.
  io.rs         The only impure code: load() and save().
  error.rs      ConfigError + ValidateErrorReason + MigrationErrorReason.
```

The two halves are clearly separated: `parse.rs` / `serialize.rs` / `migrate.rs`
/ `schema.rs` are the pure core; `io.rs` is the thin atomic leaf.

## The public API

All public surface hangs off `Config` plus the re-exported helper types. From
`lib.rs`:

```rust
pub use error::{ConfigError, MigrationErrorReason, ValidateErrorReason};
pub use schema::{
    Config, ParkedSection, PhpSection, Ports, ServiceInstance, ServicesSection, SiteOverride,
    DEFAULT_DNS_PORT,
};

pub const CURRENT_VERSION: u32 = 3;
```

`Config` exposes exactly four pure methods and two I/O methods:

```rust
impl Config {
    pub fn from_toml(s: &str) -> Result<Self, ConfigError>;
    pub fn to_toml(&self) -> Result<String, ConfigError>;
    pub fn validate(&self) -> Result<(), ConfigError>;

    pub fn load(path: &std::path::Path) -> Result<Self, ConfigError>;
    pub fn save(&self, path: &std::path::Path) -> Result<(), ConfigError>;
}
```

| Method        | Purity | Role |
|---------------|--------|------|
| `from_toml`   | pure   | parse string → version routing → wire deser → `TryFrom<Wire>` → `validate` |
| `to_toml`     | pure   | serialise to a TOML string (always writes `version = CURRENT_VERSION`) |
| `validate`    | pure   | cross-field + container-content invariants |
| `load`        | impure | read file, then `from_toml` |
| `save`        | impure | `to_toml`, then atomic write-temp-then-rename |

## The schema types

`Config` is the top-level on-disk shape:

```rust
pub struct Config {
    pub(crate) version: u32,
    pub tld: Tld,
    pub dns_port: u16,
    pub ports: Ports,
    pub php: PhpSection,
    pub parked: ParkedSection,
    pub linked: Vec<Site>,
    pub overrides: BTreeMap<String, SiteOverride>,
    pub services: ServicesSection,
}
```

Notable design decisions, all grounded in the source:

- **`version` is private.** Every `Config` produced by this build carries
  `version == CURRENT_VERSION`, so a public accessor would only ever return that
  constant. Callers read [`CURRENT_VERSION`](#schema-versioning) directly.
- **The public types implement neither `Serialize` nor `Deserialize`.**
  Round-trip goes through crate-internal *wire mirrors* (see
  [below](#wire-mirrors)). This keeps the public surface free of an accidental
  serde contract that downstream consumers might pin to.
- **`Ports`** carries `http` / `https` `u16`s. Constructors `Ports::well_known()`
  (`80 / 443`, the `Default`) and `Ports::unprivileged()` (`8080 / 8443`) are
  `const fn`.
- **`PhpSection`** holds the default `PhpVersion` (defaults to `8.3`) and a
  `BTreeMap<String, String>` of global FPM ini settings, validated against
  `yerd_core::php_settings`.
- **`ParkedSection::paths`** is a `BTreeSet<String>` - *not* `BTreeSet<PathBuf>`.
  This is deliberate: the config layer does not own platform path semantics, and
  `PathBuf::serialize` is lossy for non-UTF-8 paths on Windows. Paths are stored
  byte-exact and never canonicalised, so `"/srv/foo"` and `"/srv/foo/"` are
  distinct entries. The `BTreeSet` gives stable lexicographic order and makes
  duplicates structurally impossible.
- **`overrides`** is a `BTreeMap` keyed by a parked site's `document_root` string,
  stored byte-exact and never canonicalised. A parked site is otherwise derived
  purely from a directory listing, so it has no persistent record to hold a pinned
  PHP version, HTTPS flag, or web root; the daemon records the override here and
  re-applies it during the scan, leaving the site parked. `SiteOverride` is
  all-`Option` (`php`, `secure`, `web_root`) so `None` means "inherit" (or, for
  `web_root`, "auto-detect on every scan") and future per-site settings slot in
  additively without a wire break. `web_root` is the pinned served subdirectory
  relative to the document root - the parked-site analogue of a linked `Site`'s
  `web_subpath`.
- **`ServicesSection::instances`** is a `BTreeMap<String, ServiceInstance>` keyed
  by the service id (since the v2→v3 migration; v0–v2 stored a flat
  `enabled = [...]` array of ids). Each `ServiceInstance` carries
  `version: Option<String>`, `port: Option<u16>`, and `enabled: bool`. The keys
  are stringly-typed here on purpose: the canonical typed `Service` enum lives
  downstream in [`yerd-services`](./yerd-services), and a string key allows
  forward-compatibility with experimental services without a `yerd-config`
  release. Keys are validated against the private `KNOWN_SERVICES` const in
  `parse.rs`: `["mysql", "mariadb", "postgres", "redis"]`.

`DEFAULT_DNS_PORT` is `1053`. A fixed (non-ephemeral) port keeps the resolver
configuration installed by `yerd elevate resolver` valid across daemon restarts.

## The parse pipeline

`Config::from_toml` delegates to `parse::parse_toml`, which is the heart of the
read path:

```rust
pub(crate) fn parse_toml(s: &str) -> Result<Config, ConfigError> {
    let mut value: toml::Value = toml::from_str(s)?;
    let found = crate::migrate::read_version(&value)?;
    if found > crate::CURRENT_VERSION {
        return Err(ConfigError::UnsupportedVersion {
            found,
            current: crate::CURRENT_VERSION,
        });
    }
    if found < crate::CURRENT_VERSION {
        crate::migrate::up(&mut value, found)?;
    }
    let wire: Wire = value.try_into()?;
    let cfg = Config::try_from(wire)?;
    validate(&cfg)?;
    Ok(cfg)
}
```

The stages, in order:

1. **Lex/parse** to a generic `toml::Value` (syntax errors → `ConfigError::Parse`).
2. **Read the version** (`migrate::read_version`). A missing or non-integer
   `version` key is a hard error.
3. **Version routing.** A future version (`found > CURRENT_VERSION`) is rejected
   with `UnsupportedVersion`. An older version runs forward migrations.
4. **Wire deserialisation** into the raw `Wire` mirror.
5. **`TryFrom<Wire>`** converts raw strings into validated `yerd-core` types,
   surfacing per-field failures as `ConfigError::Core`.
6. **`validate`** runs cross-field invariants.

### Wire mirrors

The `Wire` struct is the raw shape `serde` deserialises into. It is
`#[serde(deny_unknown_fields)]` - at every level - so a typo'd key is a hard
parse error rather than a silently dropped field:

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    version: u32,
    #[serde(default = "default_tld_str")]
    tld: String,
    #[serde(default = "default_dns_port")]
    dns_port: u16,
    #[serde(default)]
    ports: PortsWire,
    // …php, parked, linked, overrides, services
}
```

The mirror is **raw-typed**: `tld`, `php.default`, and override/site `php` are
held as `String`, not as `Tld` / `PhpVersion`. This is the key design choice in
`parse.rs`. If the wire structs deserialised directly into the domain types,
`yerd-core`'s validation failures would be folded into `ConfigError::Parse` via
`serde::de::Error::custom`. By keeping the fields raw and converting in
`TryFrom<Wire>`, a bad domain value surfaces as the precise
`ConfigError::Core(..)` carrying the underlying `yerd_core::CoreError` - e.g. a
`PhpVersion` minor out of range, or a TLD containing whitespace.

`#[serde(default)]` on the section fields means an omitted `[parked]`,
`[services]`, or `[[overrides]]` block parses as empty. This matters for
forward-compatibility: a v1 file written before `overrides` existed still parses
under `deny_unknown_fields` because the field defaults rather than being required.
The same applies to the v2 additions: `SiteWire.web_subpath` and
`OverrideWire.web_root` are `#[serde(default)]`, so a migrated v1 file (which has
neither key) parses cleanly with both defaulting to "none / auto-detect".

### `TryFrom<Wire>` → Config

`TryFrom<Wire> for Config` does the raw → typed conversion:

- A post-migration sanity check asserts `wire.version == CURRENT_VERSION`; a
  `STEPS` misconfiguration that failed to bump the version surfaces here as
  `UnsupportedVersion`.
- `Tld::new`, `PhpVersion::from_str`, and `Site::linked` / `Site::parked` run the
  `yerd-core` validators. Any failure short-circuits with `ConfigError::Core`.
- The `[[overrides]]` array is folded into the path-keyed `BTreeMap`. A duplicate
  `path` (only reachable by hand-editing) is last-wins via `BTreeMap::insert`.

::: warning Silent TLD normalisation
`yerd_core::Tld::new` strips one trailing dot. So `tld = "test."` parses as
`Tld("test")`, and the next `save` emits `tld = "test"`. This is a known,
intentional silent normalisation - pinned by
`parse_strips_trailing_dot_from_tld_silently`.
:::

## Validation

`validate` enforces invariants the type system and `BTreeSet` storage cannot.
Per-field invariants on typed fields (TLD, `PhpVersion`, site name) are already
enforced during `Wire → Config`; `validate` covers cross-field and
container-content checks. The order is fixed and pinned by a test
(`validate_returns_first_failure_in_documented_order`) so the *first* failure is
deterministic:

| # | Check | `ValidateErrorReason` |
|---|-------|-----------------------|
| 1 | `ports.http == 0` | `HttpPortZero` |
| 2 | `ports.https == 0` | `HttpsPortZero` |
| 3 | `ports.http == ports.https` | `HttpHttpsPortsEqual` |
| 4 | two linked sites share a `name()` | `DuplicateLinkedSite` |
| 5 | a `parked.paths` entry is empty | `ParkedPathEmpty` |
| 6 | an `overrides` key is empty | `OverridePathEmpty` |
| 7 | a linked `web_subpath` or override `web_root` is absolute or contains `..` | `WebRootEscapes` |
| 8 | a `services.instances` key is not in `KNOWN_SERVICES` | `UnknownService` |
| 9 | a `php.settings` entry fails `php_settings::validate_value` | `InvalidPhpSetting` |

The `php.settings` check runs last (it is the newest invariant). It rejects both
unsupported directives (e.g. `allow_url_fopen`) and values that fail the shape /
security validation (e.g. `"256M; evil"`). The `WebRootEscapes` check (added with
schema v2) is the load-time containment guarantee for `web_subpath` / `web_root`:
a plain relative path is allowed, but an absolute path or one with a `..`/root
component is rejected so a hand-edited value can never make `Site::served_root`
escape the project directory.

## Schema versioning and migration

Every on-disk file **MUST** carry a top-level `version = N` key. A missing key is
a hard error (`ConfigError::Migration { MissingVersion }`), not a default. The
version is the single trigger for forward migrations.

```rust
/// The on-disk schema version this crate writes.
pub const CURRENT_VERSION: u32 = 3;
```

`CURRENT_VERSION` is **decoupled** from `yerd_ipc::PROTOCOL_VERSION`: the on-disk
TOML schema and the IPC wire protocol evolve independently. It is bumped together
with a new entry in `migrate::STEPS`.

`migrate.rs` holds the steps, indexed so that **`STEPS[N]` walks `vN → v(N+1)`**
- matching `migrate::up`, which indexes `STEPS[current]` (the version being
migrated *from*). At v3 there are three:

```rust
pub(crate) type MigrationStep = fn(&mut Value) -> Result<(), ConfigError>;

/// STEPS[N] walks vN → v(N+1); a v1 file is migrated by STEPS[1].
pub(crate) const STEPS: &[MigrationStep] =
    &[migrate_v0_to_v1, migrate_v1_to_v2, migrate_v2_to_v3];
```

`STEPS[0]` (v0→v1) is reachable only via a hand-crafted `version = 0` file - v0
was never written to disk - but it must exist so the later indices line up.
`v0→v1` and `v1→v2` are bare version bumps (v2 only **added** the optional
`web_subpath` / `web_root` keys, which default when absent). `v2→v3` is the first
**structural** step: it rewrites the old flat `services.enabled = [...]` array of
ids into per-service `[services.<id>]` tables (each previously-enabled id becomes
an `enabled = true` instance).

Each step rewrites the parsed `toml::Value` in place and is responsible for
leaving the `version` key set to `N + 1`. A step need not produce a *valid*
config - `parse_toml` unconditionally runs wire deserialisation and `validate`
after the final step, so the validator is the ultimate gate. `migrate::up` walks
`STEPS` from the found version up to `CURRENT_VERSION`; a missing step yields
`MigrationErrorReason::MissingStep { from }` (a developer error, not user input).

`read_version` defends against a non-table root (`MissingVersion`), a
non-integer or out-of-`u32`-range version (`NonIntegerVersion`), and a negative
version (also `NonIntegerVersion`).

::: tip Adding a migration
Bump `CURRENT_VERSION`, append a `MigrationStep` to `STEPS` that mutates the
`toml::Value` and sets `version = N + 1`, and never silently drop fields. The
post-migration `wire.version == CURRENT_VERSION` assertion and `validate` catch a
step that forgets to bump the version.
:::

## Serialisation and byte shape

`to_toml` routes through *borrowed* wire mirrors (`WireSer<'a>` and friends) that
hold references into the public `Config`, then calls `toml::to_string_pretty`.
The output shape is deliberate and pinned by `tests/toml_byte_shape.rs`:

- **`version` is always written first.** `WireSer.version` is the first struct
  field, and TOML emits scalars before sub-tables. The output always starts with
  `version = 3\n`.
- **Scalars precede their sibling tables.** `dns_port` is emitted as a top-level
  scalar before any `[section]`; `php.default` precedes the `[php.settings]`
  sub-table.
- **Empty optionals are omitted.** Empty `overrides` emits no `[[overrides]]`
  table; empty `php.settings` emits no `[php.settings]` sub-table; per-override
  `php` / `secure` are skipped individually when `None`.
- **Empty sets still emit `[]`.** `parked.paths` serialises as `paths = []` rather
  than being dropped; an empty `services.instances` emits no `[services.*]` tables.
- **Deterministic ordering.** `BTreeSet` / `BTreeMap` give lexicographic output,
  so `parked.paths` is sorted, `[services.<id>]` tables emit in id order, and
  `[[overrides]]` order is stable.
- **`services` wire shape** is one `[services.<id>]` table per engine, each with
  `version` / `port` (omitted when unset) and `enabled`.

A representative populated document round-trips cleanly:

```toml
version = 3
tld = "test"
dns_port = 1053

[ports]
http = 8080
https = 8443

[php]
default = "8.2"

[parked]
paths = ["docroot-a", "docroot-b"]

[[linked]]
name = "api"
document_root = "docroot"
php = "8.3"
secure = true
kind = "linked"

[[overrides]]
path = "docroot-a/blog"
php = "8.4"
secure = true

[services.mysql]
port = 3306
enabled = true

[services.redis]
version = "8"
port = 6379
enabled = true
```

(`web_subpath` on `[[linked]]` and `web_root` on `[[overrides]]` are omitted when
empty, so they don't appear in a root-served example like this one.)

## Atomic I/O (`io.rs`)

The two impure functions are intentionally minimal. `load` reads the file and
hands the string to `from_toml`, wrapping any I/O failure as `ConfigError::Io`
carrying the caller-supplied `PathBuf`. `save` is a write-temp-then-rename:

```rust
pub(crate) fn save(cfg: &Config, path: &Path) -> Result<(), ConfigError> {
    let serialised = cfg.to_toml()?;

    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    fs::create_dir_all(parent)?;                 // (errors mapped to ConfigError::Io)
    let tmp = NamedTempFile::new_in(parent)?;    // sibling temp in the same dir
    fs::write(tmp.path(), serialised.as_bytes())?;
    tmp.persist(path)?;                          // atomic rename onto destination
    Ok(())
}
```

Properties worth knowing as a contributor:

- **Atomicity.** The temp file is created *in the destination's parent dir*, so
  `persist` is a same-filesystem `rename(2)` on Unix - atomic. On Windows it is
  `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`, atomic for the rename itself but
  able to fail with `ERROR_SHARING_VIOLATION` if another process holds an
  exclusive handle to the destination. The daemon must not hold a write handle to
  the config file between saves.
- **No orphan temp files.** On `persist` failure the original `NamedTempFile` is
  dropped, which deletes the temp file.
- **Unix permissions.** `NamedTempFile` creates the file mode `0600`
  (owner read/write only); that propagates to the destination. Intentional - the
  daemon is the only intended writer.
- **No `fsync`.** Neither the file nor the parent directory is fsynced. The
  portability cost outweighs the durability gain for a developer-only config
  file; loss under sudden power loss is acceptable.
- **Parent-less paths.** `path.parent()` is `None` for a bare filename and `""`
  for some inputs; both are treated as the current working directory. So
  `Path::new("config.toml")` saves relative to the process CWD.
- **Parent dirs are created but not cleaned up.** `fs::create_dir_all` may create
  intermediate directories; they are not removed on a later failure.

## Errors

`ConfigError` is the single error type returned by every fallible public API.
It is `#[non_exhaustive]` (as are the reason sub-enums), so new variants are
semver-compatible. It is **not** `Clone`/`Eq` because it wraps `toml::de::Error`,
`toml::ser::Error`, and `std::io::Error` - and it stores the full `io::Error`
plus a `PathBuf` because diagnostic detail matters for `load`/`save` debugging.

| Variant | Meaning |
|---------|---------|
| `Parse(toml::de::Error)` | TOML failed to lex/parse syntactically |
| `Serialize(toml::ser::Error)` | serialisation failed (always a bug) |
| `Validate { reason: ValidateErrorReason }` | a cross-field / container invariant failed |
| `Core(yerd_core::CoreError)` | a domain value (TLD, `PhpVersion`, `Site`) was rejected during `TryFrom<Wire>` |
| `UnsupportedVersion { found, current }` | on-disk version incompatible (usually `found > current`) |
| `Migration { reason: MigrationErrorReason }` | version reading or forward migration failed |
| `Io { path, source }` | I/O failed in `load` / `save` |

`ValidateErrorReason` enumerates the nine checks in the
[validation table](#validation). `MigrationErrorReason` is `MissingVersion`,
`NonIntegerVersion`, or `MissingStep { from }`.

## Key tests and invariants

The crate has a coverage gate (`cargo llvm-cov … --fail-under-lines 80`, with
`tests/`, `lib.rs`, and `yerd-core` excluded). Beyond unit tests in each module,
the integration tests pin the durable contracts:

- **`tests/roundtrip.rs`** - `default` and a fully populated config survive
  `to_toml` → `from_toml`, and the populated config passes `validate`.
- **`tests/toml_byte_shape.rs`** - structural goldens on the emitted TOML: the
  `version = 3` first line, scalar-before-table ordering, omitted empty optional
  tables, `[]` for empty sets, lexicographic ordering, and the
  per-id `[services.<id>]` table shape. These survive
  `to_string_pretty`'s line-break and table-ordering choices by asserting on
  substrings and re-parsed `toml::Value`s rather than exact bytes.
- **`tests/io.rs`** - `save` → `load` round-trip, parent-dir creation, overwrite
  semantics, and that a missing file / invalid TOML surface as `Io` / `Parse`
  with the caller-supplied path.
- **`tests/io_parentless.rs`** - isolated single-test binary (it mutates the
  process CWD, which would race peer tests) confirming a parent-less path saves
  relative to the CWD.

## What it must NOT do

This crate is deliberately narrow. It does **not**:

- **Decide where the config lives.** There is no path-discovery, no
  `dirs`/`directories` dependency, no env or XDG logic. The caller (the daemon)
  passes an absolute path into `load` / `save`. See the
  [daemon page](../binaries/yerdd).
- **Scan the filesystem for sites.** Parked-site discovery is a directory scan
  performed by the daemon. `yerd-config` only persists the *parked paths* and the
  *per-path overrides* derived from that scan - it never walks directories
  itself. The only filesystem access in the whole crate is the temp file and
  rename inside `io.rs`.
- **Re-validate domain types.** TLD, `PhpVersion`, and `Site` invariants belong
  to `yerd-core`; `yerd-config` calls those validators and propagates their
  errors as `ConfigError::Core`. See [yerd-core](./yerd-core).
- **Canonicalise paths.** Parked paths and override keys are stored byte-exact.
  Callers wanting equality semantics must normalise before insertion.

## See also

- [Configuration Reference](../../reference/configuration) - user-facing field guide
- [yerd-core](./yerd-core) - the domain types this crate validates against
- [The Daemon](../../guide/daemon) - the primary reader/writer of the config
- [Crates Overview](../crates)
- Source: [`crates/yerd-config`](https://github.com/forjedio/yerd/tree/main/crates/yerd-config)
