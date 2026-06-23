---
applyTo: "bin/yerd-helper/**/*.rs"
---

# yerd-helper — the privileged one-shot

The **only** privileged component in Yerd, and therefore the security boundary.
A separate binary that does exactly one operation and exits.

**Hard rules — these are non-negotiable:**

- Validate **every** argument with a strict, typed CLI. Reject anything
  unexpected.
- **Never shell out.** No `sh -c`, no spawning external commands to do the work.
- **Never accept network input.** Arguments come from the elevating caller only.
- Do **one** operation per invocation, then exit. No daemon mode, no loop.
- Keep dependencies minimal and the code auditable.
- Check the effective UID; only the documented debug-build bypass field
  (compiled out of release via `cfg(debug_assertions)`) may skip it.

## Owns

- Clap-derived typed subcommands in `cli.rs` (e.g. install/uninstall CA, install
  resolver, setcap, port redirect) under `ops/`.
- Argument validation (`validate.rs`), privilege check (`privilege.rs`), and the
  single operation dispatch (`exec.rs`).

## Wire contract

- In debug builds the clap parse is cross-checked against
  `yerd_platform::HelperInvocation::from_argv`; a mismatch fires `WireDrift`.
  This guards against a clap upgrade silently changing argv normalisation. Keep
  the two parsers in agreement; the check is debug-gated so it can't brick
  release binaries.
- The argv shape is a contract shared with `yerd-platform`. Changing a
  subcommand's flags means updating both sides and the argv tests.

## Must not

- Grow product logic — it consumes `yerd-platform` (privileged impls), validates
  the CA PEM directly (the `pem` crate), and does the one effect.
- Pull in `anyhow` into anything but its own top-level error reporting, or any
  network/OpenSSL dependency.

## Tests / invariants

- `tests/argv_contract.rs` — the argv ↔ invocation contract.
- `tests/no_runtime_deps.rs` — forbidden crates absent.

## Review checklist

- [ ] No shelling out, no network input, one operation then exit.
- [ ] Every argument validated; effective-UID check intact (bypass debug-only).
- [ ] argv contract kept in sync with `yerd-platform`; tests pass.
- [ ] Dependency surface stays minimal and auditable.
