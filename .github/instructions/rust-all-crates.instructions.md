---
applyTo: "**/*.rs"
---

# Rust — rules for every crate

These apply to all Rust code in the workspace, on top of the repo-wide baseline.
Crate-specific files add to (never relax) these.

## Layering

- Decide a module's layer before writing it: **pure** (no I/O, sync,
  runtime-free) or **edge** (does I/O, may be async). Do not mix the two in one
  function. Where a crate has `pure/` and `io/` (or `os/`) directories, put new
  code in the matching one.
- A pure function takes data in and returns data/`Result`; it never reads the
  clock, env, filesystem, or network, and never spawns a process. If you need
  one of those, take it as a trait parameter and let the caller inject it.

## Errors

- Libraries: one `thiserror` enum per crate (`error.rs`), variants typed and
  specific. No `anyhow` in library code or in a library's dependency graph.
- Binaries: `anyhow` is allowed at the top level (`main`/command handlers) for
  context-rich exit errors; convert typed crate errors with `?`/`.context(...)`.
- Never collapse distinct failures into a generic `Internal`/`Other` when a
  caller could reasonably branch on them — add a variant.

## Forbidden in non-test code

`unsafe`, `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!`,
`unimplemented!`, `dbg!`, and slice/array indexing that can panic (`v[i]`). Use
`get(i)`, pattern matching, `?`, and total functions instead. In test files,
opt out explicitly at the top:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
```

## Async & runtime

- Only edge layers depend on `tokio`. Never add `tokio` (or any async runtime)
  to a crate or module that is supposed to be pure.
- Keep blocking work off the async executor; if a pure computation is heavy,
  it still stays sync — the caller decides how to schedule it.

## Dependencies

- Add deps via `[workspace.dependencies]` in the root `Cargo.toml` and reference
  with `name.workspace = true`. Match the existing feature-flag style
  (`default-features = false` + explicit features is the norm here).
- Respect `=` version pins; they exist for MSRV traps or to turn silent upstream
  `#[non_exhaustive]` additions into deliberate version bumps. Read the comment
  before changing one.
- Several crates ship a `tests/no_runtime_deps.rs` that walks the resolved
  dependency graph and fails if a forbidden crate (`anyhow`, `reqwest`,
  `openssl*`, `native-tls`) is reachable from the default build, or if a
  sensitive crate (`tokio`, `time`) resolves to more than one version. If you
  add a dependency, run the crate's tests — do not weaken this guard.

## Tests

- Pure logic: table-driven unit tests next to the code or in `tests/`.
- Side effects: inject a fake implementation of the trait; never perform real
  I/O in a unit test.
- Wire shapes (`serde` JSON/TOML for IPC and config) are pinned by
  `wire_stability` / byte-shape / golden tests. If one fails, do not "fix" it by
  editing the expected output unless you intend a contract change — a failure
  usually means an accidental rename or reordering.
- Add documentation on public items; `missing_docs` warns and pedantic clippy is
  enabled.

## Review checklist (for automated review)

- [ ] No new I/O, env, clock, or process access in a pure crate/module.
- [ ] No `unwrap`/`expect`/`panic`/indexing in non-test code.
- [ ] New side effect is behind a trait with a fake-backed test.
- [ ] Errors are typed (`thiserror`); no `anyhow` in a library.
- [ ] New dep added to workspace table; pins respected; `no_runtime_deps`
      still passes.
- [ ] Per-OS change is mirrored across `linux`/`macos`/`unsupported`.
- [ ] Public items documented; wire/golden tests intentionally updated, if at all.
