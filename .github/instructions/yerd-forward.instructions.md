---
applyTo: "bin/yerd-forward/**/*.rs"
---

# yerd-forward: the launchd port forwarder (macOS)

A tiny, **unprivileged** TCP relay. `yerd elevate ports` / `elevate lan` install
root-owned launchd daemons that bind 80/443 and spawn this binary **as the
invoking user** (`UserName`) in `inetdCompatibility` `Wait` mode, which passes
the listening socket on stdin. It relays each connection to the daemon's
rootless port. The plist shape lives in
`yerd_platform::pure::launchd_ports`; the helper writes it.

**Hard rules: these are non-negotiable:**

- **Never runs as root.** `main.rs` refuses euid 0 (exit 77) as a second guard
  behind the helper's uid checks. Don't remove or weaken it.
- **No `unsafe`, no FFI.** The socket comes from stdin through safe std APIs
  (`as_fd().try_clone_to_owned()` into `TcpListener`). Keep the workspace
  `forbid(unsafe_code)`.
- **Never write to stdout or stderr on the serve path.** In `Wait` mode they
  are the listening socket. Only `--version` and argument errors may print,
  before serving starts.
- **Blocking I/O.** The inherited listener and every accepted stream may be
  non-blocking (Darwin inherits `O_NONBLOCK`); call `set_nonblocking(false)` on
  both before use.
- **Upstream is the accepted local address**, port `--to`. Never hardcode
  `127.0.0.1`: in LAN scope that would keep serving LAN devices after
  `yerd lan disable`.
- **LAN scope filters connections** (`admit`): drop loopback-destined
  connections and peers that aren't `yerd_core::is_lan_source`.
- No async runtime, no network clients, no TLS: plain std threads. Keep the
  dependency graph minimal (`tests/no_runtime_deps.rs`).

## Owns

- Argument parsing (`--scope <loopback|lan> --to <port>`, `--version`), the
  `admit` filter, `upstream_for`, and the accept/relay loop in `lib.rs`.
- A thin `main.rs`: parse, refuse root, take the stdin listener, serve.

## Wire contract

- The flags and scope values are written by `yerd-helper` from
  `launchd_ports`. `tests/contract.rs` asserts the two agree and that a composed
  plist's argv parses. Change both sides together.

## Tests / invariants

- Unit tests for `parse_args`, `upstream_for`, `admit`.
- `tests/relay.rs`: real loopback sockets (echo, half-close, closed upstream,
  concurrency, non-blocking listener, LAN scope on loopback is refused).
- `tests/contract.rs`, `tests/no_runtime_deps.rs`.

## Review checklist

- [ ] Root refusal intact; no `unsafe`; nothing printed while serving.
- [ ] Listener and accepted streams forced blocking.
- [ ] Upstream follows the accepted local address; LAN filter intact.
- [ ] Flags kept in sync with `launchd_ports`; dependency surface unchanged.
