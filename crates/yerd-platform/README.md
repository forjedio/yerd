# yerd-platform

OS abstraction layer for Yerd. Houses every per-OS, often-privileged
operation behind a small trait so the daemon and helper binaries stay
testable.

## Surface

Four traits, each with macOS and Linux implementations selected by
`#[cfg(target_os = ...)]`:

- `Paths` - config / data / state / cache / runtime directories.
- `TrustStore` - install / uninstall / probe a root CA in the **system**
  trust store, plus a separately-callable Firefox/NSS per-user install.
- `ResolverInstaller` - install / uninstall / probe the per-TLD resolver
  redirect.
- `PortBinder` - bind a single TCP listener, plus an atomic 80+443 (or
  rootless 8080+8443) pair-binding helper.

Windows builds compile against `os::unsupported`, whose impls return
`PlatformError::Unsupported` for every method.

## Privilege boundary

`yerd-platform` itself is unprivileged library code. Operations that need
root (writing `/etc/resolver/<tld>`, copying into anchor directories,
applying `setcap`) return `PlatformError::NeedsHelper { operation }`. The
typed `HelperInvocation` enum carries the request to the `yerd-helper`
binary (a separate crate) for execution.

The OS impls never call `Command::new("yerd-helper")` directly. The daemon
owns the spawn; this crate owns the typed contract.

## Pure decisions

Decision logic that does not need OS interaction lives in `src/pure/*`:

- `firefox` - parse `profiles.ini`.
- `resolv_conf` - detect whether `systemd-resolved` is in charge.
- `resolver_file` - compose and parse `/etc/resolver/<tld>` (macOS).
- `resolved_drop_in` - compose and match `systemd-resolved` drop-ins.
- `port_plan` - decide rootless fallback for a port pair.
- `pem_match` - match a SHA-256 fingerprint against a list of PEM blobs.

All pure helpers are unit-tested in-memory.

## Test exemption

Each `#[cfg(test)] mod tests` opens with the workspace-standard exemption:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used,
        clippy::panic, clippy::indexing_slicing)]
mod tests { ... }
```

## Deferred to Phase 2

Windows impls, `Autostart`, `Elevation` traits, macOS LaunchDaemon FD
hand-off, FrankenPHP routing concerns. None of these change the Phase 1
trait surface.
