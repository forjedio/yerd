# yerd-forward

Unprivileged TCP forwarder that carries ports 80/443 to Yerd's rootless ports
on macOS. `sudo yerd elevate ports` (and `elevate lan`) have `yerd-helper`
install root-owned launchd daemons whose `Sockets` make launchd bind the
privileged port. On the first connection launchd starts `yerd-forward` **as the
invoking user** (`UserName`) in `inetdCompatibility` `Wait` mode, passing the
listening socket on stdin. The forwarder accepts connections and relays bytes
to the daemon. No pf is involved.

## Usage

```
yerd-forward --scope <loopback|lan> --to <port>
yerd-forward --version
```

It is only meant to be started by launchd. Run by hand, it exits because stdin
is not a socket.

## Hard rules

1. **Never root.** It refuses to run with euid 0.
2. **No `unsafe`, no FFI.** The listener comes from stdin through safe std
   APIs.
3. **Silent while serving.** In `Wait` mode stdout and stderr are the socket,
   so nothing is printed after start-up.
4. **LAN scope filters.** Connections destined for loopback, and peers outside
   private LAN ranges, are dropped.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | `--version` |
| 64 | Bad arguments |
| 66 | stdin is not a listening socket |
| 74 | I/O error while serving |
| 77 | Running as root |
| 78 | Not supported on this OS |
