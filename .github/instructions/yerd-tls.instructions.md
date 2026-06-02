---
applyTo: "crates/yerd-tls/**/*.rs"
---

# yerd-tls — local CA & leaf certificates

An mkcert-equivalent certificate authority and per-site leaf issuance,
implemented as **pure crypto** with `rcgen` / `rustls`.

**Layer:** pure. No disk, no sockets, no network. Callers pass PEM strings in
and receive PEM strings out.

## Owns

- Generating a CA key pair + self-signed root (with the CA basic constraint).
- Issuing leaf certs signed by the CA for a site plus `*.<tld>` SANs, with a
  sane validity window (`params.rs`, `leaf.rs`, `validity.rs`).
- PEM serialise/deserialise and loading an existing CA from PEM strings.
- The CA SHA-256 fingerprint, for "is this CA already trusted?" checks by the
  platform layer.

## Must not

- Install anything into a trust store — that is `yerd-platform::TrustStore`.
- Bind sockets or run a TLS server — that is `yerd-proxy`.
- Read or write files itself.
- Pull in OpenSSL / native-tls (rustls + rcgen only).

## Conventions

- `rcgen` is pinned with `=`. The `rcgen_error_detail_table_is_current` tripwire
  exists so a silent upstream addition to `rcgen::Error` becomes a deliberate
  version bump — do not bypass it; update the table and bump intentionally.
- Keep everything in-memory and deterministic enough to assert on (chains, SANs,
  validity, fingerprints).

## Tests / invariants

- `tests/chain.rs`, `tests/sans.rs`, `tests/validity.rs`, `tests/fingerprint.rs`,
  `tests/roundtrip.rs`, `tests/pem_edge_cases.rs`, `tests/issuance_negative.rs`.
- `tests/no_runtime_deps.rs` — no OpenSSL/native-tls in the graph.

## Review checklist

- [ ] No file/socket/trust-store I/O added.
- [ ] Leaf chains to the CA; SANs include the site **and** the `*.<tld>` wildcard.
- [ ] No OpenSSL variant reachable; rcgen pin/tripwire respected.
- [ ] Validity window and fingerprint behaviour covered by tests.
