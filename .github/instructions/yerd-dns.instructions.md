---
applyTo: "crates/yerd-dns/**/*.rs"
---

# yerd-dns — the `.test` resolver

Answers `*.test` (and any configured TLD) to loopback, and runs the DNS server
that does it.

**Layer split:** `responder.rs` / `answer.rs` are **pure**; `server.rs` is the
`hickory-dns` I/O edge.

## Owns

- **Pure:** given a query name + record type and the configured TLD, decide the
  answer records (`A 127.0.0.1`, `AAAA ::1`, NXDOMAIN otherwise).
- **I/O:** a `hickory-dns` server binding UDP/TCP on a loopback port that calls
  the pure responder.

## Must not

- Configure the OS resolver to point at this server — that is
  `yerd-platform::ResolverInstaller`.
- Put any answer-deciding logic inside the server layer; the server only binds,
  receives, delegates to the `Responder`, and replies.

## Conventions

- All matching logic lives in the pure responder and is table-tested
  (matching/non-matching names, A vs AAAA, subdomains, wrong TLD).
- The server keeps to one ephemeral-port integration test; do not add behaviour
  that can only be exercised by binding a real socket.

## Tests / invariants

- `tests/pure_responder_no_io.rs` — the responder is pure and exhaustively
  table-tested.
- `tests/server_smoke.rs` — one real query against an ephemeral port.

## Review checklist

- [ ] New answer logic is in the pure responder, not the server.
- [ ] No OS-resolver configuration added here.
- [ ] Pure responder tests cover the new case; server stays a thin wrapper.
