# Elevation

`elevate` and `unelevate` perform one-shot OS-level privilege setup and must be run with `sudo`. Unlike every other command, they do **not** map to a single IPC request: the CLI fetches read-only facts from your running daemon, then spawns the audited `yerd-helper` for each privileged operation. (Attempting to route them over IPC is an explicit usage error.)

| Command | Description |
| --- | --- |
| `sudo yerd elevate [TARGET]` | Grant yerd OS-level privileges. No target = grant all. |
| `sudo yerd unelevate [TARGET]` | Revert what `elevate` configured. No target = revert all. |

## Targets

| Target | Description |
| --- | --- |
| `trust` | Trust the local CA in the OS system store. |
| `resolver` | Route `*.<tld>` queries to yerd's DNS responder. |
| `ports` | Allow the daemon to bind privileged ports 80/443. |
| `lan` | Reach 80/443 from **other devices** on the LAN (see [LAN sharing](./lan)). |

```sh
sudo yerd elevate            # grant all three, in order: trust -> resolver -> ports
sudo yerd elevate trust      # just trust the local CA
sudo yerd elevate resolver   # just route *.test to the yerd DNS responder
sudo yerd elevate ports      # just allow binding 80/443
sudo yerd elevate lan        # allow LAN devices to reach 80/443 (after `yerd lan enable`)
sudo yerd unelevate          # revert everything
sudo yerd unelevate trust    # just untrust the CA
sudo yerd unelevate lan      # remove the LAN redirect (macOS)
```

With no target, `elevate`/`unelevate` apply the core three in the order `trust -> resolver -> ports`. `lan` is separate and opt-in - it is not part of "all", and you run it only after `yerd lan enable`.

::: warning Platform differences
- **Linux:** `ports` is a one-time `setcap cap_net_bind_service` grant on `yerdd`. After granting it, restart the daemon for 80/443 to take effect. There's no clean reverse operation, so `unelevate ports` only prints the manual `setcap -r` command rather than running it. Package upgrades reset `setcap`, so re-run `elevate ports` afterwards. `lan` reuses the same `setcap` grant (a wildcard bind needs the same capability), so on Linux `elevate lan` is equivalent to `elevate ports`.
- **macOS:** `ports` installs a `pf` redirect mapping 80 to the daemon's rootless HTTP port and 443 to its HTTPS port. It's live immediately (no daemon restart) and `unelevate ports` removes the redirect. `lan` installs a **separate** `pf` redirect (on your LAN IP) so other devices reach 80/443; it requires `ports` as a prerequisite for on-host access, and `unelevate lan` removes just the LAN rule.

On a host where a target isn't supported (for example `resolver` without `systemd-resolved`), that step is **skipped**, not failed, and guidance is printed.
:::

`sudo yerd uninstall` reverts all three of these (it runs the same `unelevate`) as part of removing yerd entirely - see [Uninstall](./uninstall). When removing a CA from the trust store, `yerd-helper` first confirms the matched certificate is Yerd's own (Subject CN `Yerd Local CA`) and refuses otherwise, so a mistaken fingerprint can't delete an unrelated trusted root.

The [Elevation & Privileges guide](../../guide/elevation) explains the security model and the `yerd-helper` boundary in detail.
