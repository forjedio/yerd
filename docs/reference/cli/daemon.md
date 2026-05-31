# Daemon control

| Command | Description | Example |
| --- | --- | --- |
| `yerd restart daemon` | Restart the daemon itself. | `yerd restart daemon` |

```sh
yerd restart daemon
```

::: warning
`yerd restart daemon` briefly interrupts all sites, and this command itself, since it's a client of the daemon it's restarting.
:::

There is no `yerd start`/`yerd stop` subcommand: the daemon's lifecycle is managed by your OS (and started on demand). See [The Daemon](../../guide/daemon) for how `yerdd` is launched and supervised, and the [yerdd binary page](../../developer/binaries/yerdd) for internals.
