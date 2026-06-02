---
applyTo: "xtask/**/*.rs"
---

# xtask — build automation

A Rust binary run as `cargo xtask <cmd>`: build/packaging glue, not product
code. Today it covers `.deb` packaging (`deb.rs`), bundle assembly (`pack.rs`),
and version handling (`version.rs`).

**Layer:** orchestration glue. Pure helpers are tested; the glue wires tools
together.

## Conventions

- Keep decision logic (version parsing, path/layout computation, manifest
  shaping) in pure helper functions that can be unit-tested; keep the
  shell/process orchestration thin around them.
- `.deb` packaging must reapply `setcap cap_net_bind_service=+ep` to the daemon
  in postinst/postupgrade — package upgrades reset it, and the daemon needs it to
  bind `80`/`443`. Do not drop this step.
- Cache expensive build inputs (e.g. `static-php-cli` builds, 30–60 min cold)
  aggressively; only rebuild on PHP point releases.
- Verify downloaded artifacts by SHA-256.

## Must not

- Embed product/runtime logic — `xtask` packages and automates, it does not
  implement features.
- Silently change the on-disk install layout that the binaries and platform
  `Paths` assume.

## Review checklist

- [ ] Packaging helpers are pure-tested; orchestration stays thin.
- [ ] `.deb` postinst/postupgrade reapplies `setcap`.
- [ ] Downloaded artifacts SHA-verified; caching preserved.
- [ ] No runtime/product logic leaked into build automation.
