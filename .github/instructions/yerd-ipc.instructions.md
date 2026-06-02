---
applyTo: "crates/yerd-ipc/**/*.rs"
---

# yerd-ipc — protocol & framing

The request/response protocol between clients (CLI, GUI) and the daemon, plus
the length-prefixed framing that carries it over a socket/pipe.

**Layer:** the codec is pure and runtime-free. Transport is **feature-gated**
behind `transport` (the only part allowed to touch `tokio`).

## Owns

- `Request`, `Response`, `ErrorCode`/status — internally tagged on `type`,
  `snake_case`.
- The pure frame codec (`frame.rs`): `encode_frame`, `FrameDecoder` (partial
  reads, multiple frames per buffer, oversized-length rejection), and
  `encode_message` / `decode_message`.
- Optionally, an async connect/read/write transport helper, shared by `yerdd`
  and the CLI — **only** under the `transport` feature.

## Must not

- Bind or own a socket/pipe in the default build. No async in the default
  feature set — keep the codec runtime-free so pure clients can use it.
- Reimplement domain logic; depend on `yerd-core` for domain types only.

## Contract rules (this is a published contract)

- Add variants and fields **additively**. Never rename or reorder a variant or
  field silently — `tests/wire_stability.rs` will (and should) fail.
- Expand `ErrorCode` rather than overloading a generic variant.
- Introduce/raise the protocol version constant on any breaking change, with a
  handshake before the first incompatible change.

## Tests / invariants

- `tests/frame_codec.rs` — partial, pipelined, oversized, and empty frames.
- `tests/roundtrip.rs` — message encode/decode.
- `tests/wire_stability.rs` — exact JSON tags/shapes. Treat a failure as a
  contract alarm, not a test to "fix".

## Review checklist

- [ ] Protocol change is additive; no silent rename/reorder.
- [ ] `transport`/`tokio` stays behind the feature gate; default build is pure.
- [ ] Protocol version bumped if the change is breaking.
- [ ] Wire-stability + codec tests pass and were updated intentionally.
