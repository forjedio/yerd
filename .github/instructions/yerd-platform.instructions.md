---
applyTo: "crates/yerd-platform/**/*.rs"
---

# yerd-platform — OS abstraction layer

Houses every per-OS, often-privileged operation behind a trait, with one thin
implementation per OS selected by `#[cfg(target_os = ...)]`.

**Layer:** the traits and `pure/` decision logic are pure; the `os/` impls are
the edge. This crate is itself **unprivileged** — it never elevates.

## Owns

- The core traits: `Paths`, `TrustStore`, `ResolverInstaller`, `PortBinder`,
  `PortRedirector`, and system metrics.
- One impl per OS in `os/`: `linux`, `macos`, and `unsupported`. Exactly one is
  active per build; `os/mod.rs` re-exports the active set as `Active*` aliases.
  **Windows currently compiles against the `unsupported` stub**, which returns
  `PlatformError::Unsupported` for every method — keep that stub total.
- Pure decision helpers in `pure/`: `firefox` profile discovery, `pem_match`,
  `pf_anchor`, `port_plan`, `resolv_conf`, `resolved_drop_in`, `resolver_file`,
  process/system metrics parsing.
- The typed `HelperInvocation` describing a privileged request to `yerd-helper`.

## Privilege boundary (critical)

- Operations needing root return `PlatformError::NeedsHelper` carrying a typed
  `HelperInvocation`. **The OS impls never spawn the helper themselves** — a
  privileged caller (the daemon, or `yerd elevate` under sudo) owns the
  `Command::new(...)`. Do not add a `Command` that runs the helper from inside
  this crate.

## Must not

- Contain product logic — the traits expose OS effects only.
- Elevate, shell out to perform privileged work, or embed the helper invocation.
- Put OS-specific *decisions* in the `os/` impls when they can be pure functions
  in `pure/` that the impl calls.

## Cross-platform discipline

- Any change to one OS impl must be mirrored in the others (or be a deliberate,
  commented difference). A change that only compiles on the host OS breaks CI on
  the other and the `unsupported` build.
- Prefer growing `pure/` helpers (table-tested) over logic embedded in a
  `#[cfg]` block that only one CI runner exercises.

## Tests / invariants

- `tests/linux_smoke.rs`, `tests/macos_smoke.rs`, `tests/unsupported.rs` —
  per-OS smoke of the non-privileged paths.
- `tests/helper_argv_shape.rs` — the `HelperInvocation` ↔ argv contract.
- `tests/no_runtime_deps.rs` — forbidden crates absent; single tokio/time.
- Pure parsers (`profiles.ini`, `resolv.conf`, port plans, PEM match) are
  unit-tested in-memory.

## Review checklist

- [ ] No elevation / helper-spawn added here; privileged ops return
      `NeedsHelper` with a typed invocation.
- [ ] Per-OS change mirrored across `linux`/`macos`/`unsupported`.
- [ ] Decision logic lives in `pure/` and is table-tested.
- [ ] `unsupported` stub stays total; argv-shape test still passes.
