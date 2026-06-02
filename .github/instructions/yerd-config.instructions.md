---
applyTo: "crates/yerd-config/**/*.rs"
---

# yerd-config — persisted configuration

Loads, validates, migrates, and saves Yerd's persisted state and user settings
as TOML.

**Layer:** parsing/validation/serialisation/migration are **pure**
(`parse.rs`, `schema.rs`, `serialize.rs`, `migrate.rs`). `io.rs` is the thin,
atomic edge.

## Owns

- The config schema: parked paths, linked sites, default PHP version, TLD,
  HTTP/HTTPS ports (and the rootless `8080`/`8443` variant), per-site overrides,
  enabled services.
- Pure `from_toml` / `to_toml` / `validate`, plus schema versioning and forward
  migration.
- A thin atomic `load(path)` / `save(path)` using write-temp-then-rename.

## Must not

- Decide *where* config lives — that is `yerd-platform::Paths`.
- Scan the filesystem for sites — that is the daemon's I/O layer building a
  `SiteRouter`.
- Do anything non-atomic on save (never truncate-in-place the live file).

## Conventions

- Parse/validate/migrate must remain pure and table-tested. Only `io.rs` touches
  the disk, and only via the atomic temp-then-rename pattern (including the
  parentless-path edge already covered by tests).
- The on-disk TOML byte shape is a compatibility surface; `PhpVersion` serialises
  as `"8.3"`. Keep migrations forward-compatible — old files must still load.

## Tests / invariants

- `tests/roundtrip.rs` — parse↔serialise stability.
- `tests/toml_byte_shape.rs` — exact emitted TOML; intentional changes only.
- `tests/io.rs` / `tests/io_parentless.rs` — atomic save against a temp dir,
  including missing parent directories.

## Review checklist

- [ ] No path/location knowledge added (that belongs to `yerd-platform`).
- [ ] Save stays atomic; no in-place truncation.
- [ ] Schema change ships a migration; old configs still load.
- [ ] Byte-shape test updated only for an intended format change.
