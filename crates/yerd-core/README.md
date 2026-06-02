# yerd-core

Pure domain model and host→site routing for [Yerd](../../).

## What's in it

- `PhpVersion` — strict major.minor with `Display`/`FromStr` and a custom serde impl that wires through TOML/JSON as the canonical string `"8.3"`.
- `Tld` — validated DNS suffix newtype (ASCII, lowercased, DNS-label rules).
- `Site` / `SiteKind` — a routable target with a private `name` invariant and typed setters (no `set_name` — renaming is a router operation).
- `RouterConfig` — typed TLD plus a cached `".{tld}"` suffix for the hot path.
- `SiteRouter` — `BTreeMap`-backed registry with `new`, `from_sites`, `insert`, `remove`, `get`, `get_mut`, `iter`, `len`, `is_empty`, `config`, and the host→site `resolve` algorithm.
- `CoreError` (+ `*Reason` enums) — single error type, every variant `#[non_exhaustive]`.

## Purity rules

This crate is **pure**:

- No I/O (no filesystem, no network, no process spawning, no clock, no env).
- No `tokio`, no async, no other `yerd-*` internal deps.
- No `unwrap`/`expect`/`panic!`/indexing slicing in non-test code (clippy-enforced via the `[lints.clippy]` block).
- Side effects belong behind traits in `yerd-platform` and similar adapters.

If a future task wants to add I/O here, **stop** — it belongs behind a trait elsewhere.

## `document_root` invariant

`Site::document_root` is **not** validated by `yerd-core`. It may be empty, relative, or non-canonical. Path semantics, existence, and platform normalisation are owned by `yerd-config` (load time) and `yerd-platform` (runtime). Round-trip through `serde` uses `PathBuf`'s default string representation, which is lossy for paths that cannot be encoded as UTF-8 (notably Windows paths containing unpaired surrogates from WTF-16). Callers needing a guaranteed-UTF-8 path should normalise upstream.

## Test exemption policy

Every `#[cfg(test)] mod tests { … }` block in `src/` opens with:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests { … }
```

Integration files under `tests/` use the equivalent crate-level inner attribute. The restriction lints apply to production code only; idiomatic tests use `unwrap`/`expect`/`assert!`/indexing freely.

## Coverage

The crate targets **≥ 80%** line coverage (actual ≈ 88–93%). Measured with `cargo-llvm-cov` (cross-OS, stable). The pinned toolchain in `/rust-toolchain.toml` includes `llvm-tools-preview` for this.

```sh
cargo install cargo-llvm-cov                 # first-time install
cargo llvm-cov --package yerd-core --lib --tests \
    --fail-under-lines 80 \
    --ignore-filename-regex '/tests/'
```

## Local gate (until cross-OS CI lands)

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo llvm-cov --package yerd-core --fail-under-lines 80
```

