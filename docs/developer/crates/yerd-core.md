# yerd-core

`yerd-core` is the foundation of the Yerd workspace. It holds the **pure domain
model** - the validated value types that describe PHP versions, sites, and TLDs
- and the **host→site routing** algorithm that maps an incoming HTTP `Host:`
header to a registered site. Every other crate in the workspace depends on it,
directly or transitively.

The crate's defining property is its purity: **no I/O, no async, no internal
`yerd-*` dependencies, and `unsafe` is forbidden at the crate level.** Side
effects (reading config files, spawning processes, touching the filesystem or
the network) live behind traits in adapter crates such as
[`yerd-platform`](./yerd-platform). That separation is what lets the domain
logic be exhaustively unit-tested without mocks, and lets the same types travel
across the IPC boundary unchanged. See the [Crates Overview](../crates) for how
this fits the wider workspace, and the [IPC Protocol](../ipc-protocol) page for
the shared wire types these definitions pin.

## Crate metadata

```toml
[package]
name        = "yerd-core"
description = "Pure domain model and host→site routing for Yerd."

[dependencies]
serde     = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
serde_json = { workspace = true }
serde_test = { workspace = true }
toml       = { workspace = true }
```

The only runtime dependencies are `serde` (for wire serialisation) and
`thiserror` (for the error enum). There are no internal dependencies - this is
the bottom of the dependency graph. The crate root opens with:

```rust
#![forbid(unsafe_code)]
```

`unsafe_code` is also forbidden workspace-wide (`[workspace.lints.rust]
unsafe_code = "forbid"`), so the crate-level attribute is belt-and-braces.

## Module map

The crate splits into focused modules; only a curated surface is re-exported
from `lib.rs`:

```rust
pub mod detect;
mod error;
mod host;
mod php;
pub mod php_settings;
mod router;
mod site;
mod tld;

pub use detect::{detect, Detection, ProjectSignals};
pub use domain::{choose_primary, effective_domains, Domain, DomainErrorReason};
pub use error::{CoreError, PhpVersionErrorReason, SiteNameErrorReason, TldErrorReason};
pub use php::PhpVersion;
pub use php_settings::{PhpSettingError, ValueErrorReason};
pub use router::{RouterConfig, SiteRouter};
pub use site::{Site, SiteKind};
pub use tld::Tld;
```

| Module          | Visibility    | Responsibility                                            |
| --------------- | ------------- | --------------------------------------------------------- |
| `error`         | re-exported   | `CoreError` + the typed `*Reason` sub-enums               |
| `php`           | re-exported   | `PhpVersion` `(major, minor)` value type                  |
| `site`          | re-exported   | `Site` / `SiteKind`                                       |
| `tld`           | re-exported   | `Tld` validated DNS-suffix newtype                        |
| `domain`        | re-exported   | `Domain` sub-part + `effective_domains` / `choose_primary` algebra |
| `router`        | re-exported   | `RouterConfig` + `SiteRouter` (the resolve algorithm)     |
| `php_settings`  | `pub mod`     | managed PHP ini directives + value validation             |
| `detect`        | `pub mod`     | pure web-root detection (`ProjectSignals` → `Detection`)  |
| `host`          | `pub(crate)`  | `Host:` header normalisation, consumed only by `resolve`  |

`php_settings` is a `pub mod` (callers reach functions as
`yerd_core::php_settings::validate_value(...)`); `host` is deliberately
crate-private - only `SiteRouter::resolve` consumes it.

## `PhpVersion`

A PHP `major.minor` pair. Both fields are `pub`, but the constructor that
accepts user input is `FromStr` - `new` is unchecked and intended for code
constructing known-good versions.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PhpVersion {
    pub major: u8,
    pub minor: u8,
}

impl PhpVersion {
    #[must_use]
    pub const fn new(major: u8, minor: u8) -> Self { /* ... */ }
}
```

Deriving `Ord` on `(major, minor)` gives **numeric**, not lexicographic,
ordering: `(8, 9) < (8, 10) < (8, 99)`. `Display` emits the dotted form
(`"8.3"`).

### Parsing

`FromStr` (and therefore `Deserialize`) runs a pinned three-step algorithm:

1. **Leading-byte classification.** A non-ASCII first byte (letter, emoji,
   zero-width space, CJK) is rejected uniformly as `UnsupportedPrefix`. An ASCII
   letter is only accepted if the first three bytes are a case-insensitive
   `"php"` and nothing alphabetic follows - so `"php8.3"`, `"PHP8.3"`,
   `"Php8.3"` parse, but `"phpa8.3"`, `"v8.3"`, `"py8.3"` are
   `UnsupportedPrefix`. Anything else (digit, punctuation, whitespace) passes
   through to step 2.
2. **Shape.** `rest` must match `DIGIT+ "." DIGIT+`. A missing `.` is
   `MissingMinor`; an empty side or any non-digit byte is `NonNumeric`.
3. **Range.** Parts are parsed as `u16` so overflow is classified uniformly:
   `major ∈ 5..=9` else `MajorOutOfRange`; `minor ∈ 0..=99` else
   `MinorOutOfRange`; values ≥ 65536 that overflow `u16` are `NonNumeric`.

Parsing is total and never panics on multibyte input - the algorithm operates
on bytes and only does char-boundary-safe slicing. Round-tripping is
canonicalising: `"8.00"` parses to `(8, 0)` and displays as `"8.0"`.

### Serde shape

`PhpVersion` serialises as a **string** via `collect_str`, and deserialises
through a custom visitor that rejects JSON numbers and ints:

```rust
assert_tokens(&PhpVersion::new(8, 3), &[Token::Str("8.3")]);   // frozen
serde_json::from_str::<PhpVersion>("8.3").is_err();            // number → error
serde_json::from_str::<PhpVersion>("8").is_err();              // int → error
```

::: tip
The string representation is what makes the type safe to embed in TOML config
and JSON IPC payloads alike - a bare `8.3` in TOML would be a float and lose the
distinction between `8.10` and `8.1`.
:::

## `Site` and `SiteKind`

A `Site` is a routable target with a validated name, a document root, a served
web subpath, a PHP version, an HTTPS flag, and a kind. Fields are **private** to
enforce the name invariant; the type exposes accessors and typed setters.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteKind {
    Parked,   // auto-discovered under a parked directory
    Linked,   // explicitly registered
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    name: String,
    document_root: PathBuf,
    web_subpath: PathBuf,   // served web root, relative to document_root ("" = root)
    php: PhpVersion,
    secure: bool,
    kind: SiteKind,
}
```

Construction goes through `Site::parked` or `Site::linked`, both of which
validate and lowercase the name and **initialise `secure = false`** (promote
with `set_secure`):

```rust
pub fn parked(name: &str, document_root: impl Into<PathBuf>, php: PhpVersion)
    -> Result<Self, CoreError>;
pub fn linked(name: &str, document_root: impl Into<PathBuf>, php: PhpVersion)
    -> Result<Self, CoreError>;
```

Accessors: `name() -> &str`, `document_root() -> &Path`,
`web_subpath() -> &Path`, `php() -> PhpVersion`, `secure() -> bool`,
`kind() -> SiteKind`. Setters: `set_document_root`, `set_web_subpath`,
`set_php`, `set_secure`, `set_kind`.

`served_root() -> PathBuf` is the directory the proxy actually serves:
`document_root` when `web_subpath` is empty (avoiding a stray `join("")`
trailing separator), else `document_root.join(web_subpath)`. It is **defensive by
construction - it can never escape the document root**: an absolute or
`..`-bearing `web_subpath` (which `Path::join` would let climb out) falls back to
serving the document root. The authoritative containment check is in `yerd-config`
at load time; `served_root` is the second line of defence because `Site` itself
does no path validation.

There is **no `set_name`**. The name is immutable after construction because it
doubles as the router's lookup key - renaming is a router-level remove/reinsert
operation, not a field mutation. This is what makes `SiteRouter::get_mut` safe:
a caller can mutate any field of a stored site without the routing key drifting.

### Name validation

`validate_and_lowercase_name` runs a pinned, ordered algorithm. The order is
load-bearing and is pinned by tests - e.g. `ContainsDot` (step 2) beats
`LabelTooLong` (step 6), and `LeadingOrTrailingHyphen` (step 5) beats length:

1. empty → `Empty`
2. contains `.` → `ContainsDot` (sites are single DNS labels)
3. byte outside `[A-Za-z0-9-]` (including any non-ASCII or whitespace) →
   `InvalidCharacter`
4. lowercase
5. leading/trailing `-` → `LeadingOrTrailingHyphen`
6. length > 63 bytes → `LabelTooLong` (RFC 1035 single label; byte length equals
   char length because step 3 rejected non-ASCII)

### Document root is *not* validated

```rust
// document_root is **not** validated by yerd-core - this is a pure crate.
```

`document_root` may be empty, relative, or non-canonical. Path semantics,
existence, and platform normalisation belong to `yerd-config` (load time) and
[`yerd-platform`](./yerd-platform) (runtime). Serde uses `PathBuf`'s default
string representation, which is lossy for paths that are not valid UTF-8;
callers needing a guaranteed-UTF-8 path must normalise upstream.

### Serde shape

`Site` has a hand-written `Serialize` and a `Deserialize` that routes through a
private `Wire` struct with `#[serde(deny_unknown_fields)]` and then re-runs name
validation/lowercasing. `web_subpath` is **skipped when empty** (the struct
serialises 5 fields then, 6 when a subpath is set), so the byte shape for a
root-served site is byte-identical to before the field existed:

```json
{"name":"foo","document_root":"/srv/foo","php":"8.3","secure":false,"kind":"parked"}
{"name":"app","document_root":"/srv/app","web_subpath":"public","php":"8.3","secure":false,"kind":"linked"}
```

The `Wire` deserialiser marks `web_subpath` `#[serde(default)]`, so a payload
written before the field existed (no `web_subpath` key) parses as the empty
subpath. Deserialisation lowercases the name, rejects invalid names, and rejects
*other* unknown fields - so config files and IPC payloads can't smuggle an
unvalidated site past the type boundary.

## `Tld`

A validated, lowercased, ASCII-only DNS-suffix newtype used by the router for
TLD enforcement.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Tld(String);

impl Tld {
    pub fn new(s: &str) -> Result<Self, CoreError>;   // validate + lowercase
    #[must_use] pub fn as_str(&self) -> &str;
}

impl Default for Tld {                 // yields the canonical ".test"
    fn default() -> Self { Self(String::from("test")) }
}
```

`Default` returns the canonical `test` TLD - the *only* hard-coded TLD
construction in the crate, and a test (`default_is_test_and_matches_new`) walks
every validation step against `"test"` so a future tightening of `validate`
can't silently break the default.

Validation (`Tld::new` / `FromStr` / `Deserialize`) is again a pinned algorithm:
reject empty; strip one trailing `.` then reject leading/trailing/empty dots;
cap total length at 253 bytes; reject non-ASCII and whitespace; lowercase; then
per-label checks (no empty labels → `ConsecutiveDots`, ≤ 63 bytes, `[a-z0-9-]`
only, no leading/trailing hyphen). Multi-label suffixes such as `"dev.local"`
are valid. Serde shape is a plain string (`"test"`).

## `php_settings`

A pure module modelling the small, fixed set of global PHP ini directives Yerd
manages. These values flow **unescaped** into every installed version's FPM pool
config as `php_value[...]` / `php_flag[...]` lines, so `validate_value` is the
**security boundary** against config injection. It runs when a value is set
(CLI + daemon), when config is loaded (`yerd-config`), and defensively again at
render time ([`yerd-php`](./yerd-php)).

The allowlist (extend here to support more directives):

| Directive             | Kind                          | Renders as  |
| --------------------- | ----------------------------- | ----------- |
| `memory_limit`        | byte size (`-1` allowed)      | `php_value` |
| `max_execution_time`  | non-negative integer          | `php_value` |
| `max_input_time`      | non-negative integer          | `php_value` |
| `max_file_uploads`    | non-negative integer          | `php_value` |
| `upload_max_filesize` | byte size                     | `php_value` |
| `post_max_size`       | byte size                     | `php_value` |
| `display_errors`      | flag (boolean)                | `php_flag`  |
| `error_reporting`     | int or constant expression    | `php_value` |

Public surface:

```rust
pub fn is_supported(name: &str) -> bool;
pub fn supported_names() -> Vec<&'static str>;       // declaration order
pub fn directive(name: &str) -> Option<&'static str>; // "php_flag" | "php_value"
pub fn validate_value(name: &str, value: &str) -> Result<(), PhpSettingError>;
pub fn canonical_value(name: &str, value: &str) -> String;
```

`validate_value` enforces a global invariant *before* the per-kind shape:
non-empty / not all-whitespace, `≤ 256` bytes, no control characters, and none
of the FPM/ini metacharacters `[ ] = ; #`. The per-kind validators are
hand-rolled (no `regex` dependency): byte sizes accept `\d+[KMGkmg]?` (plus `-1`
only when `allow_unlimited`), integers accept ASCII digits, flags accept
`on|off|1|0|true|false` case-insensitively, and `error_reporting` accepts an
integer or a constant expression limited to `[A-Za-z0-9_ &|~^()-]`.
`canonical_value` normalises validated booleans to `On`/`Off` and trims the
rest.

::: warning Security boundary
Injection attempts - embedded newlines, `;`, `#`, `]`, `=`, or over-length
input - are all rejected here. Downstream renderers re-validate, but this is the
first and primary gate.
:::

## `detect`

A pure module that decides which subdirectory of a PHP project is its web root.
It is the *decision* half of framework detection; the *I/O* half - reading
`composer.json` and stat-ing marker files - lives in
[`yerd-platform`](./yerd-platform), which feeds this function a `ProjectSignals`.

```rust
pub struct ProjectSignals {
    pub composer_requires: BTreeSet<String>,   // lowercased composer package names
    pub markers: BTreeSet<String>,             // present root markers: "artisan", "wp-config.php", …
    pub web_dirs_with_index: BTreeSet<String>, // candidate dirs containing index.php
}

pub struct Detection { pub subpath: PathBuf, pub resolved: bool } // "" = serve root

pub fn detect(sig: &ProjectSignals) -> Detection;

pub const WEB_DIR_CANDIDATES: &[&str]; // ["public", "web", "webroot", "pub"]
pub const ROOT_MARKERS: &[&str];       // the marker files/dirs the gatherer probes
```

`detect` runs a first-match-wins precedence: Laravel / Symfony (4+) /
CodeIgniter 4 → `public`, CakePHP → `webroot`, Drupal (Composer) / Yii2 → `web`,
Magento 2 → `pub`, WordPress / plain-PHP → root, then a generic
"first candidate web dir with an `index.php`" fallback, then root.

`Detection::resolved` is the signal the daemon's filesystem watcher keys on: it
is `true` for every confident branch (a framework or web dir was identified, or
the project is a confident root like WordPress) and **`false` only for the
no-evidence fallback** - an empty folder served at root provisionally. The daemon
keeps watching the unresolved ones so a project cloned in later is picked up. The
shared `WEB_DIR_CANDIDATES` / `ROOT_MARKERS` consts keep the pure decider and the
platform gatherer from drifting on what to probe. Every branch is table-tested
with fixture `ProjectSignals` (precedence, mixed signals, empty-project default).

## `Domain` and the effective-set algebra

A `Domain` is the **sub-part** of a routable host - everything left of the
configured TLD. For TLD `test`: `foo`, `api.foo`, `*.foo`, `*.api.foo`. Storing
the sub-part (not the FQDN) is canonical because the router strips the TLD before
matching, and it keeps the value TLD-agnostic. A leftmost `*` label is a
**single-label wildcard**. `Domain` deliberately derives no serde - config
persists it through wire mirrors, and IPC carries FQDN strings.

```rust
pub struct Domain { /* private: the validated sub-part */ }

impl Domain {
    #[must_use] pub fn apex(name: &str) -> Self;                 // a site's default exact domain
    pub fn parse(fqdn: &str, tld: &str) -> Result<Self, CoreError>;   // strips + checks the TLD
    pub fn parse_subpart(sub: &str) -> Result<Self, CoreError>;       // from stored config
    #[must_use] pub fn as_str(&self) -> &str;                    // the stored sub-part
    #[must_use] pub fn is_wildcard(&self) -> bool;              // leftmost label is '*'
    #[must_use] pub fn to_fqdn(&self, tld: &str) -> String;    // sub-part + '.' + tld
}
```

Two free functions own the pure "effective set" algebra a site's routable domains
are computed with (`implicit_default ± delta`):

- `effective_domains(name, added, suppressed) -> Vec<Domain>` = `({apex} - suppressed) + added`, de-duplicated, apex-first. There is **no** implicit subdomain catch-all; the default set is just the apex. Zero-exact normalization restores the apex if a hand-edited config would otherwise leave only wildcards, so a site is always reachable under one concrete host.
- `choose_primary(name, effective, stored) -> Domain` picks the canonical, displayed domain: the stored primary if it is exact and still present, else the apex if present, else the first exact. A wildcard is never a primary.

`DomainErrorReason` (on `CoreError::InvalidDomain`) enumerates the shape failures
(`Empty`, `EmptyLabel`, `MisplacedWildcard`, `BareWildcard`, `NotUnderTld`,
`TooLong`, `LabelTooLong`, `InvalidCharacter`, `LeadingOrTrailingHyphen`).

## `SiteRouter` and `RouterConfig`

`RouterConfig` pairs a `Tld` with a **cached** `".{tld}"` suffix used on the hot
path. Its invariant - `dotted_tld == format!(".{}", tld.as_str())` - is upheld
by construction only; there is no field-by-field constructor.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterConfig {
    tld: Tld,
    dotted_tld: String,   // cache; NEVER serialised
}

impl RouterConfig {
    pub fn new(tld: &str) -> Result<Self, CoreError>;   // validates
    #[must_use] pub fn with_tld(tld: Tld) -> Self;      // pre-computes dotted_tld
    #[must_use] pub fn tld(&self) -> &str;
    #[must_use] pub fn tld_typed(&self) -> &Tld;
}
```

Serde emits exactly one field, `tld` (`{"tld":"test"}`); the `dotted_tld` cache
is never serialised, and `Deserialize` rebuilds it via `RouterConfig::new` with
`deny_unknown_fields`.

`SiteRouter` keeps a `BTreeMap<String, Site>` keyed by `site.name()` (for
identity and ordered iteration), plus - since the multi-domain feature - two
`HashMap` routing indices built from every site's effective domain set: `exact`
(sub-part → name) and `wildcards` (`*.rest` → name), and per-site `domains` /
`primaries` maps. `Default` is **deliberately not derived** - callers must pass a
config consciously rather than relying on an implicit `"test"`.

```rust
impl SiteRouter {
    #[must_use] pub fn new(config: RouterConfig) -> Self;
    pub fn from_sites(config: RouterConfig, sites: impl IntoIterator<Item = Site>)
        -> Result<Self, CoreError>;                       // first dup aborts
    pub fn insert(&mut self, site: Site) -> Result<(), CoreError>;   // DuplicateSite
    // Insert a site with an explicit effective domain set + chosen primary; the
    // colliding-key safety net returns DuplicateSite / DuplicateDomain.
    pub fn insert_with_domains(&mut self, site: Site, effective: Vec<Domain>, primary: Domain)
        -> Result<(), CoreError>;
    pub fn remove(&mut self, name: &str) -> Result<Site, CoreError>; // SiteNotFound
    #[must_use] pub fn get(&self, name: &str) -> Option<&Site>;
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Site>;
    pub fn iter(&self) -> impl Iterator<Item = &Site> + '_;          // name order
    #[must_use] pub fn len(&self) -> usize;
    #[must_use] pub fn is_empty(&self) -> bool;
    #[must_use] pub fn config(&self) -> &RouterConfig;
    #[must_use] pub fn resolve(&self, host: &str) -> Option<&Site>;
    // Domain accessors used by the daemon's DTO/doctor surfaces:
    #[must_use] pub fn domain_owner(&self, domain: &Domain) -> Option<&str>;
    #[must_use] pub fn primary_domain(&self, name: &str) -> Option<&Domain>;
    #[must_use] pub fn effective_domains(&self, name: &str) -> Option<&[Domain]>;
    #[must_use] pub fn apex_shadowed_by(&self, name: &str) -> Option<&str>;
}
```

Because the map is a `BTreeMap`, `iter` yields sites in lexicographic name order
deterministically. `get_mut` is invariant-safe precisely because `Site` has no
`set_name`. The plain `insert` gives a site only its default apex;
`insert_with_domains` is how the daemon installs a customised domain set.

### The `resolve` algorithm

`resolve` maps a raw `Host:` header value to at most one site. It first
normalises the host (via the private `host` module), then does an exact lookup
followed by a **single-label wildcard** lookup. There is **no** implicit
subdomain catch-all: an uncustomised site answers only its exact apex.

**Step 1 - host normalisation** (`host::normalise`), returning either a clean
hostname or `Unroutable`:

```
1. empty                              → Unroutable
2. starts with '['  (IPv6/bracketed)  → Unroutable
3. any non-ASCII byte                 → Unroutable
4. strip port via rsplit_once(':'):
     - no ':'                         → keep
     - tail empty or all-digits       → strip it
     - otherwise (junk tail)          → Unroutable
5. empty after strip                  → Unroutable
6. strip one trailing '.'  (FQDN)     → if now empty, Unroutable
7. starts with '.'                    → Unroutable
8. lowercase if any uppercase; else borrow (Cow)
```

The `Cow` return means the common already-normalised case (`"foo.test"`)
allocates nothing.

**Step 2 - routing**, on the normalised hostname:

```
let sub = host.strip_suffix(".{tld}")?;     // must end with the TLD; None otherwise
if host == tld          → None              // bare TLD has no site label
if sub.is_empty()       → None
if exact.get(sub)       → Some              // exact match beats wildcard
// single-label wildcard: replace the LEFTMOST label with '*', one lookup only
if let Some((_, rest)) = sub.split_once('.') {
    if wildcards.get(&format!("*.{rest}"))  → Some
}
None
```

Exact is always tried before the one wildcard candidate, so exact beats wildcard.
A wildcard matches exactly one label: `*.foo` (stored for a site) answers
`api.foo.test` but never `x.api.foo.test` (which needs `*.api.foo`). With only
`foo` registered, `api.foo.test` is unresolved (404). This lets `foo.test` and
`*.foo.test` belong to two **different** sites.

The behaviour is pinned by the router's table tests. Representative rows (site
`foo` registered with default apex only, unless a domain is noted):

| Host                | Resolves to | Rule                                      |
| ------------------- | ----------- | ----------------------------------------- |
| `foo.test`          | `foo`       | exact apex                                |
| `foo.test:8443`     | `foo`       | port stripped                             |
| `foo.test:abc`      | -           | port junk → unroutable                    |
| `[::1]:8080`        | -           | IPv6 literal                              |
| `foo.test.`         | `foo`       | trailing FQDN dot stripped                |
| `FOO.TEST`          | `foo`       | case-insensitive                          |
| `föö.test`          | -           | non-ASCII                                 |
| `foo.example`       | -           | wrong TLD                                 |
| `test`              | -           | bare TLD                                  |
| `api.foo.test`      | -           | no implicit catch-all (apex-only default) |
| `api.foo.test`      | `foo`       | when `*.foo` is registered on `foo`       |
| `x.api.foo.test`    | -           | single-label wildcard doesn't nest        |
| `foo..test`         | -           | embedded empty label                      |
| `foo.dev.local`     | `foo`       | multi-label custom TLD                    |

::: info
`resolve` returns `&Site`, so a hit gives the caller the full record -
document root, served web root (`served_root()`), PHP version, and `secure` flag -
needed to dispatch the request. The proxy and daemon hold a `SiteRouter` and call
`resolve` per request.
:::

## Error model

`CoreError` is the single error type for every fallible public API in the crate.
Each variant carries a typed `*Reason` sub-enum so callers match on precise
failure modes without parsing message strings. Every error enum is
`#[non_exhaustive]`, so new variants are semver-compatible additions.

```rust
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreError {
    InvalidPhpVersion { input: String, reason: PhpVersionErrorReason },
    InvalidTld        { input: String, reason: TldErrorReason },
    InvalidDomain     { input: String, reason: DomainErrorReason },
    DuplicateSite     { name: String },
    DuplicateDomain   { domain: String },
    SiteNotFound      { name: String },
    InvalidSiteName   { name: String, reason: SiteNameErrorReason },
}
```

`DuplicateDomain` is the router's colliding-key safety net (mirroring
`DuplicateSite`), returned by `insert_with_domains` when two domains map to the
same routing key; the daemon feeds a pre-de-conflicted set so it never fires in
production.

`php_settings` has its own pair, `PhpSettingError` (`Unsupported` /
`InvalidValue`) with `ValueErrorReason`. All error types are `Send + Sync +
Clone + Eq`, which lets them cross the IPC boundary and be compared in tests.

## Tests and invariants

Each module carries exhaustive unit tests (reason-by-reason validation tables,
ordering pins, serde-token assertions). Two **integration tests** under
`tests/` guard the cross-crate contract:

- **`serde_roundtrip.rs`** - proves the public types compose into the shapes
  downstream crates need. A `ConfigShape { php, tld, sites }` round-trips
  through TOML (mirroring what `yerd-config` loads), and an
  `IpcSetPhp { name, version }` round-trips through JSON (mirroring a `yerd-ipc`
  request payload). It asserts the human-readable forms, e.g.
  `php = "8.3"` and `tld = "test"` in TOML.

- **`wire_stability.rs`** - pins byte-exact JSON shapes for the public types.
  Renaming any public field, variant, or type name fails this file, which fails
  CI before [`yerd-ipc`](./yerd-ipc) can ship a divergent wire format:

  ```rust
  // PhpVersion          → "8.3"
  // SiteKind::Parked     → "parked"   (snake_case)
  // Site                 → {"name":"foo","document_root":"/srv/foo","php":"8.3","secure":false,"kind":"parked"}
  // RouterConfig::default → {"tld":"test"}
  ```

Together these make `yerd-core` the place where the wire contract is *defined
and frozen*, not merely consumed.

## Design boundaries - what `yerd-core` does *not* do

- **No I/O or async.** It never reads config, spawns FPM, or touches DNS - those
  live in `yerd-config`, [`yerd-php`](./yerd-php), [`yerd-dns`](./yerd-dns), and
  [`yerd-platform`](./yerd-platform).
- **No path validation.** `document_root` correctness is a load-time / runtime
  concern.
- **No `unsafe`.** Forbidden at the crate root and workspace-wide.
- **No implicit defaults that matter operationally.** `SiteRouter` has no
  `Default`; the only hard-coded value is the `.test` TLD via `Tld::default`.

## Source

Browse the crate on GitHub:
[`crates/yerd-core`](https://github.com/forjedio/yerd/tree/main/crates/yerd-core).

See also: [Crates Overview](../crates) · [IPC Protocol](../ipc-protocol) ·
[Architecture](../architecture) · [Cross-Platform Model](../cross-platform).
