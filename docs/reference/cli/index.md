# CLI Reference

The `yerd` command is a thin client that talks to the `yerdd` daemon over a local IPC socket. Almost every subcommand maps to exactly one daemon request: `yerd` validates your arguments locally, sends the request, and renders the daemon's reply as either a human-readable block or machine-readable JSON.

This reference documents every command, subcommand, positional argument, and flag exactly as the CLI defines them. If a flag isn't listed here, it doesn't exist.

::: tip
`yerd --help` and `yerd <command> --help` always print the authoritative, version-matched usage for your installed build. This reference mirrors that surface and explains what each command does behind the scenes.
:::

## Synopsis

```sh
yerd [--json] <COMMAND> [ARGS...]
```

### Global flags

| Flag | Description |
| --- | --- |
| `--json` | Emit machine-readable JSON instead of human-readable text. Available on every command. |
| `--help`, `-h` | Print help for the command. |
| `--version`, `-V` | Print the `yerd` version. |

`--json` is a global flag, so you can place it before or after the subcommand: `yerd --json status` and `yerd status --json` are equivalent. In JSON mode the entire daemon response is printed as pretty JSON; the process exit code still reflects success or failure (see [Exit codes](#exit-codes)).

::: info
`yerd` is the command-line front end. The daemon (`yerdd`) does the real work: running the proxy, DNS responder, PHP-FPM pools, and certificate authority. See [The Daemon](../../guide/daemon) for how it runs, and the [IPC Protocol](../../developer/ipc-protocol) for the request/response wire format.
:::

## Commands

| Group | Commands |
| --- | --- |
| [Sites](./sites) | `sites`, `park`, `unpark`, `link`, `unlink` |
| [HTTPS](./https) | `secure`, `unsecure` |
| [PHP](./php) | `use`, `install php`, `uninstall php`, `update php`, `restart php`, `list php`, `set php`, `unset php` |
| [Diagnostics](./diagnostics) | `ping`, `status`, `doctor`, `doctor fix` |
| [Elevation](./elevation) | `elevate`, `unelevate` |
| [Daemon control](./daemon) | `restart daemon` |

## Exit codes

`yerd` returns a meaningful process exit code so it composes cleanly in scripts and CI:

| Code | Meaning |
| --- | --- |
| `0` | Success. |
| `1` | The daemon returned an error response, or a `doctor` run had a `Fail`-severity finding. |
| `2` | Client-side usage error (bad site name, invalid PHP version, unknown/invalid PHP setting). |
| `69` | The daemon was unreachable (for non-`doctor` commands). |
| `74` | Other transport / I/O failure. |

For the `elevate`/`unelevate` path, additional codes can surface: `77` if not run as root, `69` if the daemon's facts can't be fetched, `74` if the helper/daemon sibling binaries can't be located, and `1` if any privileged step failed.

```sh
# Use the exit code in a script
if yerd doctor; then
  echo "yerd is healthy"
else
  echo "yerd reported problems (exit $?)"
fi
```

## JSON output

Pass `--json` to get the raw daemon response as pretty-printed JSON, ideal for scripting or for the [desktop app](../../guide/desktop-app) and other tooling:

```sh
yerd --json status
yerd --json list php --available
yerd --json sites
```

The exit code in JSON mode matches the human path exactly, including doctor's `Fail`-aware behaviour, so you can branch on the code and parse the body independently.

## See also

- [Sites](../../guide/sites): parking vs. linking
- [PHP Versions](../../guide/php-versions): installing, switching, and tuning PHP
- [HTTPS & Certificates](../../guide/https): securing sites
- [Elevation & Privileges](../../guide/elevation): what `sudo yerd elevate` does
- [Configuration Reference](../configuration): config file keys and locations
- [IPC Protocol](../../developer/ipc-protocol): the request/response surface each command maps to
- Source: [`forjedio/yerd`](https://github.com/forjedio/yerd)
