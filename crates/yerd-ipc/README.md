# yerd-ipc

The request/response protocol, framing, and codec between `yerdd` (the daemon) and its clients (the `yerd` CLI and the Tauri GUI).

## What's in it

- `PROTOCOL_VERSION` - bump on any breaking wire change. A handshake will land before the first breaking change; until then a mismatch surfaces as `IpcError::Decode`.
- `Request` / `Response` / `ErrorCode` - internally tagged on `type`, snake_case. `#[non_exhaustive]` so new variants are additive.
- `encode_frame` / `FrameDecoder` / `DEFAULT_MAX_FRAME` - pure length-prefixed frame codec (4-byte BE `u32` length, 16 MiB default cap).
- `encode_message` / `decode_message` - thin `serde_json` wrappers.
- `FrameError` (pure, `Clone + Eq`) and `IpcError` (with `kind()` → `IpcErrorKind` for GUI/Tauri callers that need `Clone + Eq`).
- `types` re-exports of `yerd_core::{Site, PhpVersion, SiteKind}` so clients can depend on `yerd-ipc` alone.
- Optional `transport` feature: `write_message`, `read_frame`, `read_message` over `tokio::io::AsyncRead`/`AsyncWrite`. Socket and named-pipe binding stays in the binaries.

## Purity rules

The **default build is pure**: no sockets, no async, no I/O.

- No `tokio` in the default build (it's `optional = true` and only pulled by `--features transport`).
- No `tracing` anywhere - the binaries own logging and wrap calls into `yerd-ipc` themselves.
- No `unwrap` / `expect` / `panic!` / indexing in non-test code (workspace clippy gate).

## Wire-stability policy

The `Request` / `Response` / `ErrorCode` JSON shapes are the **published contract**. The rules:

1. **Add variants and fields additively.** Never rename a variant, field, or `ErrorCode`. `#[non_exhaustive]` keeps the door open for additions without a major bump.
2. **No per-field `#[serde(rename = "...")]`.** Let `#[serde(rename_all = "snake_case")]` do the casing. The CI gate (see "Local gate" below) greps `crates/yerd-ipc/src/` for per-field renames inside `#[serde(...)]` attributes and fails on any match. Reason: pairing the no-rename rule with `tests/wire_stability.rs` and the inline `variant_name_pinning` modules means a Rust variant rename trips both the wire pin (changed JSON) and the in-crate exhaustive match (compile error) - without the rule, an attacker-of-future-self could mask a Rust rename with a `#[serde(rename = "...")]` and silently break clients.
3. **Tag-stability is enforced.** `tests/wire_stability.rs` pins byte-exact JSON for every variant. `src/request.rs` and `src/response.rs` contain inline `variant_name_pinning` modules whose exhaustive `match` arms catch any Rust variant rename at compile time (integration tests can't, because `#[non_exhaustive]` blocks exhaustive matching across crate boundaries).
4. **`PROTOCOL_VERSION` exists but is dead until a handshake lands.** Don't grow it without paired `Hello` / `Welcome` variants.
5. **Fail-closed on unknown `ErrorCode` and unknown `type` tags.** Both surface as `IpcError::Decode`. No `#[serde(other)] Unknown` to silently downgrade.

## Outer-envelope vs inner-Site asymmetry

The outer envelope (`Request` / `Response`) **accepts unknown JSON fields** (serde default) so additive field changes stay backward-compatible. The inner `yerd_core::Site` payload is **strict** (`deny_unknown_fields` on its `Deserialize` impl) - unknown fields on a `Site` inside `Response::Sites { sites }` are rejected. This asymmetry is deliberate and is asserted in `tests/roundtrip.rs` (`decode_accepts_unknown_envelope_field` and `decode_rejects_unknown_field_inside_site`).

Example: `{"type":"ping","__extra":42}` decodes as `Request::Ping`. `{"type":"sites","sites":[{"name":"foo",...,"surprise":1}]}` fails to decode.

## Workspace-deps convention

When inheriting from `[workspace.dependencies]` via `workspace = true`, extra `features` are **additive** over the workspace base; you cannot re-enable `default-features` from a consumer (feature unification across the dependency graph defeats the override). If you need defaults, set them at the workspace level.

This is why this crate's `[dev-dependencies]` adds `["macros", "rt", "io-util"]` on top of the workspace base (`default-features = false, features = ["io-util"]`) for the async smoke tests.

## Test layout

- **Inline (`src/`)** - `error.rs`, `frame.rs`, `lib.rs`, `request.rs`, `response.rs`, and the feature-gated `transport.rs` each carry `#[cfg(test)] mod tests` (or `mod variant_name_pinning`) blocks. These modules open with the standard `#[allow(...)]` block since the workspace clippy gate denies `unwrap_used` / `expect_used` / `panic` / `indexing_slicing` even in tests.
- **Integration (`tests/`)** -
  - `frame_codec.rs` covers framing edge cases: partial reads, pipelined frames, oversized rejection, decoder poisoning, exact-max boundary, slow-loris byte-at-a-time.
  - `wire_stability.rs` pins byte-exact JSON for every variant.
  - `roundtrip.rs` covers `encode_message` ∘ `decode_message` identity and the negative tests for unknown tags / unknown fields / unknown error codes.

## Local gate

```sh
cargo build -p yerd-ipc
cargo build -p yerd-ipc --features transport
cargo test  -p yerd-ipc
cargo test  -p yerd-ipc --features transport
cargo fmt   --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
! grep -REn '#\[serde\([^)]*[^_[:alnum:]]?rename[[:space:]]*=' crates/yerd-ipc/src/
```
