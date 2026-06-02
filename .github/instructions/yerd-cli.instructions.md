---
applyTo: "bin/yerd/**/*.rs"
---

# yerd — the CLI

A `yerd-ipc` **client** of the daemon. It maps commands to IPC requests, renders
results, and may auto-start the daemon if it isn't running.

**Layer:** thin client. No daemon logic, no runtime state ownership.

## Owns

- Command parsing and the pure command→`Request` mapping (`cli.rs`, `map.rs`) —
  this mapping is unit-testable without a daemon.
- The IPC client transport wiring (`transport.rs`) using `yerd-ipc`'s
  `transport` feature.
- `yerd elevate` (`elevate.rs`): when run under sudo, it owns the
  `Command::new(...)` that invokes `yerd-helper` with a typed `HelperInvocation`
  from `yerd-platform`.

## Conventions

- Human-readable output by default; `--json` for scripting. Keep the two paths
  rendering the same underlying `Response` — don't compute different data per
  format.
- The command surface is designed fresh for this product; do not port a prior
  tool's command vocabulary or assume compatibility with it.

## Must not

- Reimplement anything the daemon owns (routing, supervision, config authority).
  If the CLI needs a new capability, add a `Request`/`Response` to `yerd-ipc`
  and let the daemon implement it.
- Perform privileged effects inline beyond owning the explicit `yerd elevate`
  helper invocation.

## Tests / invariants

- `tests/cli_e2e.rs` — end-to-end against a daemon.
- The command→`Request` mapping is covered by pure unit tests.

## Review checklist

- [ ] New capability flows through an IPC request, not a local reimplementation.
- [ ] `--json` and human output derive from the same response.
- [ ] command→`Request` mapping is pure and tested.
- [ ] Elevation limited to the explicit, typed helper invocation.
