# yerd-update

`yerd-update` is the **decision layer** behind Yerd's self-update: given the
running version and a set of fetched GitHub releases, it decides whether a
newer version is available on the configured channel, and which downloadable
artifact matches the host platform. It powers the bare `yerd update` command
(see [Self-Update](../../reference/cli/update)) and the GUI's Updates section.

The crate does **no I/O**: fetching releases from GitHub, downloading
artifacts, and installing them all live in [`yerdd`](../binaries/yerdd) (the
version check) and [`yerd`](../binaries/yerd) (the applier, via
[`yerd-service-ctl`](./yerd-service-ctl)). This crate only decides *what*
should happen, given data the I/O layer already fetched.

::: info Crate metadata
`description`: *Pure release-channel selection and version-decision logic for
Yerd self-update.* `#![forbid(unsafe_code)]`. No internal `yerd-*`
dependencies - only `semver`, `sha2`, `hex`, and `minisign-verify`. A `pacman`
feature (off by default) flips [`PkgFormat::current`] from `Deb` to `Pacman`
for the Arch package build; `bin/yerdd`'s own `pacman` feature forwards into
it (`yerd-update/pacman`).
:::

See also the [Crates overview](../crates), [`yerd-service-ctl`](./yerd-service-ctl)
(the daemon-restart step of the applier), and the
[Self-Update CLI reference](../../reference/cli/update).

## Module map

```text
src/
├── lib.rs        # Channel, ReleaseMeta, UpdateDecision, select_target, is_check_due
└── artifact.rs   # Platform, PkgFormat, ArtifactKind, select_asset, verify_sha256, verify_minisign
```

[Browse the source on GitHub.](https://github.com/forjedio/yerd)

## Channel resolution: stable vs. edge

`select_target` takes the full set of fetched releases, the configured
[`Channel`], and the running version, and returns an [`UpdateDecision`]:

- **`Channel::Stable`** resolves to the highest version among releases that
  are *not* pre-releases.
- **`Channel::Edge`** resolves to the highest version across every release,
  pre-releases included.
- A release counts as a pre-release if either the source (GitHub) flagged it,
  **or** its semver carries a pre-release component (`-rc.N`) - so a
  mis-flagged release is still classified safely.
- The channel's latest only becomes `target` when it is strictly newer than
  `current`; otherwise `target` is `None` ("nothing to do"). This means the
  stable channel never *downgrades* a user running a newer pre-release -
  `UpdateDecision::ahead_of_stable` flags exactly that case, which drives the
  `--stable`/`--force` downgrade guard in the CLI.

`is_check_due` decides whether enough wall-clock time has passed
(`CHECK_INTERVAL_SECS`, 4 hours) since the last check to poll again; `yerdd`
uses it to gate its periodic background poll.

## Artifact selection and verification

`select_asset` picks the right downloadable artifact (plus its detached
signature and the `SHA256SUMS` manifest) out of a release's assets, by
filename convention: the macOS artifact ends `.app.tar.gz`, the Linux artifact
ends `.deb` or `.pkg.tar.zst` depending on [`PkgFormat`]. Intel macOS and any
other [`Platform::Unsupported`] host get `AssetError::NoArtifactForPlatform`
rather than a mis-selected file.

Verification is two independent checks, both pure functions over
already-downloaded bytes:

- `verify_sha256` looks up the artifact's filename in the `SHA256SUMS` body
  and compares against a freshly computed digest.
- `verify_minisign` checks a detached, **prehashed** minisign signature
  against one of two embedded public keys - [`UPDATE_PUBLIC_KEY`] for app
  artifacts, and the separate [`PHP_LISTING_PUBLIC_KEY`] for the PHP listing
  manifest fetched by `yerd-php` (a distinct key, since PHP installs are on
  the install critical path, not just app updates).

`yerdd`'s `StageUpdate` handler runs both checks before writing the artifact
to disk; the applier ([`yerd`](../binaries/yerd)) re-runs the minisign check a
second time immediately before swapping, closing the TOCTOU window between
daemon-verify and install.

## Public API

Re-exported from `lib.rs`:

```rust
pub use artifact::{
    select_asset, sha256_for, sha256_hex, verify_minisign, verify_sha256, ArtifactKind,
    ArtifactSelection, AssetError, PkgFormat, Platform, VerifyError, PHP_LISTING_PUBLIC_KEY,
    UPDATE_PUBLIC_KEY,
};
```

| Item | Layer | Role |
|------|-------|------|
| `Channel` | pure | `Stable` / `Edge`, with `as_str`/`parse` mirroring the persisted config string. |
| `ReleaseMeta` / `Asset` | pure | Plain data the I/O layer maps GitHub's release JSON into. |
| `select_target(releases, channel, current)` | pure | Decide the update target, latest-per-channel, and `ahead_of_stable`. |
| `UpdateDecision` | pure | The full result of `select_target`. |
| `is_check_due(last_checked, now)` | pure | Whether the 4-hour poll interval has elapsed. |
| `Platform` / `PkgFormat` | pure | Host platform and Linux package format (build-time, via a Cargo feature). |
| `select_asset(release, platform, format)` | pure | Resolve the artifact + signature + checksums asset triple. |
| `verify_sha256` / `verify_minisign` | pure | Checksum and signature verification over in-memory bytes. |
| `UPDATE_PUBLIC_KEY` / `PHP_LISTING_PUBLIC_KEY` | - | Embedded minisign public keys. |
