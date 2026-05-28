# yerd-tls

Local CA and per-site leaf certificates for [Yerd](../../).

## What's in it

- `CertAuthority` — generate a fresh CA or load one from PEM; expose
  `cert_pem` / `key_pem` / `cert_der` / `fingerprint_sha256`; issue leaf
  certs signed by the CA.
- `LeafCert` — `cert_pem` / `key_pem` accessors plus a `chain_pem` helper
  for `rustls`-style inline chains.
- `Validity` — explicit `not_before` / `not_after`; no clock reads inside
  the crate; year-9998 policy guard.
- `TlsError` + `GenerateErrorReason` / `ParseErrorReason` /
  `ValidityErrorReason` — single error type, every variant
  `#[non_exhaustive]`.

## Purity rules

This crate is **pure**:

- No I/O (no filesystem, no network, no process spawning, no clock, no env).
- No `tokio`, no async, no other `yerd-*` internal deps.
- No `unwrap` / `expect` / `panic!` / indexing-slicing in non-test code
  (clippy-enforced via the workspace `[lints.clippy]` block).
- Side effects belong behind traits in `yerd-platform` and similar adapters.

The `time = "=0.3.36"` and `rcgen = "=0.13.2"` pins live in the workspace
`Cargo.toml`; bump together with MSRV (and update the
`rcgen_error_detail_table_is_current` tripwire when rcgen moves).

## Validity windows

`Validity::new(not_before, not_after)` validates `not_before <= not_after`
and rejects either endpoint with `year() > 9998`. The 9998 ceiling reserves
a one-year gap below `time`'s representable upper bound so callers can't
accidentally emit `99991231235959Z` GeneralizedTime that some trust stores
mis-parse. No `Validity::days(n)` convenience — the daemon's policy module
owns the chosen window.

## SANs and TLDs

`issue_leaf(names, validity)` takes the SAN set verbatim. The crate is
TLD-agnostic; callers compose `[format!("{site}.{tld}"), format!("*.{tld}")]`
themselves. Each name must be a valid `IA5String` (ASCII).

## Fingerprint semantics

`fingerprint_sha256` returns the SHA-256 of the cached cert DER bytes. It is
stable across `from_pem` round-trip because `from_pem` caches the input PEM
and decoded DER verbatim. Matches what trust-store inspectors print.

## Cryptography posture

- ECDSA-P256 via `ring` (rcgen's default under our feature set, pinned via
  `KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)`).
- 20-byte random serials (rcgen default, sourced from the configured crypto
  backend's RNG).
- No `Drop`-time key zeroisation. rcgen does not zeroize; flagged here for a
  future hardening pass if/when relevant.
- `CertAuthority: Send + Sync` is guaranteed today **only under the `ring`
  feature**. A switch to `aws-lc-rs` or a remote-signer path needs
  re-verification. The compile-time `assert_send_sync_cert_authority`
  inline test pins today's state — it is a snapshot, not a future-proof
  tripwire.

## CA provenance support

Tested with `yerd-tls`-generated CAs. CAs from other tools (mkcert,
OpenSSL) are accepted by rcgen's parser provided the subject uses
single-attribute RDNs; multi-AVA RDN subjects (`CN=foo+OU=bar`) are
rejected. Leaves' AKI mirrors the loaded CA's SKI extension exactly —
vendor-specific SKI methods round-trip correctly.

## Coverage

```sh
cargo llvm-cov --package yerd-tls --lib --tests \
    --fail-under-lines 80 \
    --ignore-filename-regex '(/tests/|src/lib\.rs$)'
```

Local gate:

```sh
cargo build  -p yerd-tls
cargo test   -p yerd-tls
cargo fmt    --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo llvm-cov --package yerd-tls --fail-under-lines 80
```

## Test-exemption policy

Every `#[cfg(test)] mod tests` block in `src/` opens with:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests { … }
```

Integration files under `tests/` use the equivalent crate-level inner
attribute. Workspace lint policy denies these in non-test code.
