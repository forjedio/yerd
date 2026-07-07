# Self-Update

The bare `yerd update` command (no subcommand) checks for, and optionally
installs, a newer version of Yerd itself. `yerd update php` is a different
command that updates an installed PHP version - see [PHP](./php).

| Command | Description |
| --- | --- |
| `yerd update` | Check for a newer Yerd on your configured channel and report it. Installs nothing. |
| `yerd update --yes` | Check, then download, verify, and install the update, and restart the daemon. |
| `yerd update --edge` / `--stable` | Check (or, with `--yes`, apply) against the edge or stable channel for this run. |
| `yerd update --edge --yes` / `--stable --yes` | Also persist the given channel as your saved default. |
| `yerd update --yes --force` | Allow a downgrade (e.g. moving from a newer pre-release back to stable). |

```sh
yerd update                    # check only, using your saved channel
yerd update --edge              # check against edge, without changing your saved channel
yerd update --yes               # check and install on your saved channel
yerd update --edge --yes        # switch to (and install from) the edge channel
```

## Channels

Yerd ships two release channels, persisted as `update_channel` in
[`yerd.toml`](../configuration) (`"stable"` by default):

- **`stable`** - the highest released version that isn't a pre-release.
- **`edge`** - the highest released version including pre-releases / release
  candidates.

`--edge`/`--stable` override the saved channel **for this invocation only**.
Add `--yes` and the override also becomes the new saved default - a
check-only run (no `--yes`) never touches your saved preference, but prints a
reminder that it's showing a channel other than the one it will use next time.

The stable channel never installs a **downgrade**: if you're running a
pre-release that is already newer than the latest stable, `yerd update --yes`
(without `--edge`) reports you're ahead of stable and stays put. `--force`
(which requires `--yes`) is the escape hatch for that case, though an
automated downgrade isn't implemented yet - it currently just says so.

## What `--yes` does

1. Persists the channel override, if `--edge`/`--stable` was given.
2. Asks the daemon to check the channel (`yerd_ipc::Request::CheckUpdate`). If
   nothing is available, it reports and exits `0` - no download happens.
3. Otherwise asks the daemon to download and verify the target release's
   artifact (`Request::StageUpdate`): the daemon fetches the platform-specific
   asset (macOS `.app.tar.gz`, Linux `.deb`/`.pkg.tar.zst`/`.rpm`), checks its
   SHA-256 against the release's `SHA256SUMS` manifest, and verifies a
   detached minisign signature against an embedded public key.
4. Re-verifies the signature a second time locally (closing the window
   between the daemon's verify and the install), then installs the artifact
   and restarts the daemon so it comes back up on the new version.

`--yes` is not compatible with `--json` - the apply path is interactive
progress output, not a single structured response.

::: warning Where Yerd must live
On macOS, self-update only works when Yerd is installed at
`/Applications/Yerd.app` and that location is writable by you. Installs
elsewhere (e.g. a dev build) are rejected with a clear error rather than
silently failing.
:::

## Platform support

| Platform | Install mechanism |
| --- | --- |
| macOS (Apple Silicon) | Extracts the `.app.tar.gz`, swaps it into `/Applications`, restarts the daemon via `launchctl kickstart -k`. |
| Linux (`.deb` build) | Reinstalls via `dpkg -i`, elevated with `pkexec` if needed; restarts the daemon via `systemctl --user restart` (or a stop/start fallback with no systemd user instance). |
| Linux (Arch `.pkg.tar.zst` build) | Reinstalls via `pacman -U`, same elevation and restart path as the `.deb` build. |
| Linux (Fedora `.rpm` build) | Reinstalls via `rpm -U --oldpackage`, same elevation and restart path as the `.deb` build. |
| Intel macOS, other platforms | No self-update artifact is published; `yerd update` reports nothing available. |

## See also

- [PHP](./php) - the unrelated `yerd update php [VERSION]` command.
- [Configuration Reference](../configuration) - the persisted `update_channel` key.
- [yerd-update](../../developer/crates/yerd-update) - the channel-resolution and artifact-selection/verification logic.
- [yerd-service-ctl](../../developer/crates/yerd-service-ctl) - the daemon restart step of the applier.
- [Daemon control](./daemon) - the separate `yerd restart daemon` command.
