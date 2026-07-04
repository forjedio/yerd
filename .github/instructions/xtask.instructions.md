---
applyTo: "xtask/**/*.rs"
---

# xtask — build automation

A Rust binary run as `cargo xtask <cmd>`: build/release glue, not product code.
Today it covers exactly two things — version **`bump`** (set the version across
the three manifests) and **`version-check`** (the release gate). Pure helpers
live in `version.rs`; the I/O glue lives in `main.rs`.

Packaging is **not** an xtask concern: the single GUI bundle (`.dmg` on macOS,
`.deb` on Linux) has the three binaries embedded via `externalBin`
(per-platform overlays in `apps/yerd-gui/src-tauri/`). Tauri builds the `.app`
(macOS) and `.deb` (Linux) directly; the macOS `.dmg` is packaged as a
separate headless step (`apps/yerd-gui/scripts/build-macos-dmg.sh`, via
`appdmg`) rather than by Tauri's own dmg bundler, which drives Finder via
AppleScript and isn't reliable outside an interactive session. The Linux
`.deb`'s `setcap`/symlink `postinst` lives in
`apps/yerd-gui/src-tauri/deb/postinst.sh`, not here. (xtask used to build a
standalone `.deb`; that subcommand and its `deb.rs`/`pack.rs`/`assets/` were
removed.)

**Layer:** orchestration glue. Pure helpers are tested; the glue wires the
manifest edits together.

## Conventions

- Keep decision logic (version parsing/normalising, manifest editing/reading,
  the sync assertion) in pure helper functions (`version.rs`) that are
  unit-tested in-memory; keep the file I/O thin around them.
- Version edits must stay **surgical** — rewrite only the single `version` line
  in each manifest, preserving indentation, key, trailing comma, and trailing
  newline. Never reformat the whole document.
- The three manifests that must never drift: `Cargo.toml`
  (`[workspace.package].version`), `apps/yerd-gui/src-tauri/tauri.conf.json`, and
  `apps/yerd-gui/package.json`.

## Must not

- Embed product/runtime logic — `xtask` automates the release, it does not
  implement features.
- Re-introduce artifact building/packaging or runtime downloads — those belong to
  Tauri (bundles) and the GitHub Actions workflow (signing, checksums).

## Review checklist

- [ ] Version logic is pure-tested in `version.rs`; `main.rs` only does I/O.
- [ ] Manifest edits are surgical (single line, formatting preserved).
- [ ] `version-check` covers all three manifests.
- [ ] No packaging/download/product logic leaked into build automation.
