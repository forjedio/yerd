# yerd-platform

OS abstraction layer for Yerd. Houses every per-OS, often-privileged
operation behind a small trait so the daemon and helper binaries stay
testable.

## Surface

The core traits, each with a single thin implementation per OS selected by
`#[cfg(target_os = ...)]`:

- `Paths` - config / data / state / cache / runtime directories.
- `TrustStore` - install / uninstall / probe a root CA in the **system**
  trust store, plus a separately-callable Firefox/NSS per-user install.
- `ResolverInstaller` - install / uninstall / probe the per-TLD resolver
  redirect.
- `PortBinder` - bind a single TCP listener, plus an atomic 80+443 (or
  rootless 8080+8443) pair-binding helper.
- `PortRedirector` - install / remove the privileged-port redirect.
- `TerminalLauncher` - open the host terminal at a directory.
- `IdeLauncher` - detect and launch the host editor.
- `SystemOpener` - hand a path or URL to the desktop.

macOS and Linux implement all of them. Windows has real `Windows*` impls for
`Paths`, `TrustStore`, `ResolverInstaller`, `PortBinder`, `PortRedirector` and
`TerminalLauncher`, and aliases `IdeLauncher`, `SystemOpener` and
`SystemMetrics` to the `os::unsupported` stub, which returns
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
- `resolv_conf` - conservatively select systemd-resolved, NetworkManager, or unsupported.
- `networkmanager_dnsmasq` - compose and match NetworkManager dnsmasq snippets.
- `dns_probe` - compose the loopback DNS probe and validate its answer.
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

## Outstanding

Three traits are still aliased to the `unsupported` stub on Windows:
`IdeLauncher`, `SystemOpener` and `SystemMetrics`. Each is replaced by a real
`Windows*` type in the same change that adds its full trait impl, never
half-flipped.
