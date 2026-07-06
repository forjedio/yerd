# yerd-helper

Privileged one-shot binary for Yerd. The daemon (`yerdd`) runs
unprivileged; operations that require root - installing the CA into the
system trust store, writing resolver redirects, applying
`cap_net_bind_service` - are pushed across this binary as the single
security boundary.

## Subcommands

- `install-ca --pem <path> --fingerprint <64hex>`
- `uninstall-ca --fingerprint <64hex>`
- `install-resolver --tld <name> --addr <socketaddr>`
- `uninstall-resolver --tld <name>`
- `setcap --binary <path>` (Linux only)

The argv shape is the frozen wire contract pinned by
`yerd-platform`'s `helper_argv_shape.rs` and `helper_argv_roundtrip.rs`
tests. The daemon calls `HelperInvocation::to_argv()` to produce argv;
the helper calls `HelperInvocation::from_argv()` (plus a clap layer for
`--help`/`--version`) to consume it.

## Hard rules

1. **Strict typed args, no shell.** Every external command uses
   `Command::new(...).env_clear()` with a pinned `PATH` and arguments
   added one at a time. No `sh -c`, no string interpolation.
2. **Defence in depth.** The helper does not trust the daemon. It
   re-validates the PEM fingerprint against the PEM contents, re-parses
   the TLD against `yerd-core::Tld`, checks paths are absolute and exist,
   and refuses any operation it cannot complete safely.
3. **One operation, then exit.** No long-running state, no network, no
   environment-driven config beyond `PATH`.

## Exit codes

Follows `sysexits.h`:

| Code | Meaning |
|---|---|
| 0 | success |
| 64 | bad argv structure (`EX_USAGE`) |
| 65 | bad argv data: invalid hex, addr, etc. (`EX_DATAERR`) |
| 69 | required external tool missing (`EX_UNAVAILABLE`) |
| 70 | internal contract bug - wire drift (`EX_SOFTWARE`) |
| 74 | I/O failed (`EX_IOERR`) |
| 75 | external command failed transiently (`EX_TEMPFAIL`) |
| 77 | not running privileged (`EX_NOPERM`) - daemon should retry with elevation |
| 78 | OS / config mismatch - `Unsupported` (`EX_CONFIG`) |

## Manual privileged-op test recipes

The CI suite does not run the actual privileged operations. Verify
manually:

### Linux

```bash
sudo target/debug/yerd-helper install-ca \
  --pem /tmp/ca.pem \
  --fingerprint $(sha256sum < /tmp/ca.der | cut -d' ' -f1)

# Confirm:
ls /usr/local/share/ca-certificates/yerd-*.crt

sudo target/debug/yerd-helper uninstall-ca \
  --fingerprint $(sha256sum < /tmp/ca.der | cut -d' ' -f1)
```

### macOS

```bash
sudo target/debug/yerd-helper install-ca \
  --pem /tmp/ca.pem \
  --fingerprint <64hex>

# Confirm:
security find-certificate -Z /Library/Keychains/System.keychain \
  | grep -i <64hex>

sudo target/debug/yerd-helper uninstall-ca --fingerprint <64hex>
```

## Deferred

- Windows: a stub `main` exits 78. Real Windows behaviour lands in
  Phase 2 with `yerd-platform`'s Windows impls.
- macOS Authorization Services entitlements wrapper (Phase 2).
- Linux distro variants outside Debian/Ubuntu/Alpine/RHEL/Fedora/Arch
  (anchor-dir auto-detect returns `Unsupported` for unknown layouts).
- `tracing` subscriber - `eprintln!` to stderr is enough for a
  millisecond-lifetime one-shot.
