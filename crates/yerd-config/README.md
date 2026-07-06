# yerd-config

Persisted TOML configuration for Yerd. Owns the parse / validate / serialise
pipeline for the on-disk config file, plus a thin atomic load / save.

## Purity

Every function in this crate except `Config::load` and `Config::save` is
pure: no filesystem, no network, no environment reads. Tests at the pure
layer use string inputs only.

## Schema versioning

Every on-disk file MUST carry a top-level `version = N` key. A missing key
is a hard error (`ConfigError::Migration { MissingVersion }`). The version
is the single trigger for forward migrations. `CURRENT_VERSION` is bumped
together with a new entry in `migrate::STEPS` - never silently drop fields.

## I/O semantics

`save` writes to a sibling temp file via `NamedTempFile::new_in(parent)`,
then `persist`s onto the destination. On Unix this is `rename(2)` (atomic
when src and dst share a filesystem - guaranteed because the temp is
created in the destination's parent dir). On Windows this is `MoveFileExW`
with `MOVEFILE_REPLACE_EXISTING` - atomic for the rename itself, but can
fail with `ERROR_SHARING_VIOLATION` if another process holds an exclusive
handle to the destination.

Unix files end up with mode 0600 (owner read/write only) inherited from
the temp file. The daemon is the only intended writer; broader permissions
are the operator's call to set after install.

`save` does not `fsync` the file or parent directory. Loss under sudden
power loss is acceptable for a developer-only config file. Intermediate
parent directories may be created via `fs::create_dir_all`; they are not
removed on a later failure. A parent-less path (e.g. `Path::new("config.toml")`)
is treated as relative to the process's current working directory.

## Path storage

`ParkedSection::paths` is `BTreeSet<String>`, not `BTreeSet<PathBuf>`. The
config layer does not own platform-specific path semantics, and
`PathBuf::serialize` is lossy for non-UTF-8 paths on Windows. Strings are
stored verbatim - no canonicalisation. `"/srv/foo"` and `"/srv/foo/"` are
distinct entries. Callers convert to `PathBuf` at the point of use.

## TLD normalisation

`yerd_core::Tld::new` strips one trailing dot. Therefore `tld = "test."`
parses as `Tld("test")` and the next save emits `tld = "test"`. This is a
known silent normalisation.

## Coverage

```
cargo llvm-cov --package yerd-config --lib --tests \
    --fail-under-lines 80 \
    --ignore-filename-regex '(/tests/|src/lib\.rs$|yerd-core)'
```

The `yerd-core` exclusion keeps this gate scoped to `yerd-config`'s own
source files - `yerd-core` has its own coverage gate.

Requires `cargo-llvm-cov` ≥ 0.5 for `--fail-under-lines`.

## Test-exemption policy

Every `#[cfg(test)] mod tests` block opens with:

```rust
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
```

Each `tests/*.rs` integration file uses the equivalent crate-inner attribute.
The workspace lint policy denies these in non-test code.
