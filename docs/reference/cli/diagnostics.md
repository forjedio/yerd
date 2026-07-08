# Status & Diagnostics

| Command | Description | Example |
| --- | --- | --- |
| `yerd ping` | Check that the daemon is alive (prints `pong`). | `yerd ping` |
| `yerd status` | Show a snapshot of daemon, proxy, DNS, ports, CA, and PHP health. | `yerd status` |
| `yerd doctor` | Diagnose common problems and report findings. | `yerd doctor` |
| `yerd doctor fix` | Attempt safe, unprivileged repairs (e.g. restart a crashed FPM pool). | `yerd doctor fix` |

```sh
yerd ping            # pong, if the daemon is up
yerd status          # one-screen health snapshot
yerd doctor          # report problems and remedies
yerd doctor fix      # apply safe automatic fixes, then list what still needs you
```

`yerd status` reports the daemon PID, uptime and RSS, version, TLD, the bound HTTP/HTTPS ports (flagging rootless fallback or an active macOS pf redirect), the DNS responder address, CA trust state and path, resolver install state, load average, site counts, and a per-version PHP pool listing (state, PID, listen socket, RSS, available update).

`yerd doctor` prints each finding with a severity mark (`✓` ok, `⚠` warn, `✗` fail), a detail line, and a `→` remedy where one exists. `yerd doctor fix` first lists what it applied, then what still needs manual attention.

One such finding is `DomainShadowed` (a `Warn`): two sites claim the same domain, so one site's apex is shadowed by the other. The remedy is to `yerd domain remove` the duplicate or `yerd domain primary` the shadowed site onto a free domain.

::: info Exit codes for diagnostics
`yerd doctor` (and `yerd doctor fix`) exit `1` if any finding is `Fail` severity, otherwise `0`. A `Warn` alone does **not** fail the exit code. This holds in both human and `--json` modes, so doctor is safe to use in CI gates.

If the daemon is unreachable, `yerd doctor` is special-cased: instead of the generic "daemon unreachable" error, it surfaces a synthetic `Daemon not running` **Fail** finding and exits `1`, so a down daemon shows up as a doctor failure, consistently across `--json` and the exit code.
:::

See the [Diagnostics guide](../../guide/diagnostics) for an explanation of each check.
