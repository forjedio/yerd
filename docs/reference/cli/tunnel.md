# Tunnel

The `yerd tunnel` commands publish a local site to the public internet through
[Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/)
(`cloudflared`). The connection is outbound-only - no inbound ports, no `sudo`.
The [Sharing Sites guide](../../guide/sharing) covers the model and the two tiers
(quick tunnels and named tunnels); this page is the command reference.

::: info cloudflared is detected on PATH, or downloaded on demand
The first share needs the `cloudflared` binary. Yerd looks for one already on
your `PATH` first and uses it if it's version 2023.3.0 or newer; otherwise
`yerd tunnel install` fetches the official static build (checksum-verified) into
the daemon's data directory - it is not bundled and never touches your system
unless you already had a compatible `cloudflared` installed. Quick share needs no
Cloudflare account; named tunnels need a one-time `yerd tunnel login`.
:::

## Commands

| Command | Description |
| --- | --- |
| `yerd tunnel install` | Download the `cloudflared` binary (required before sharing). |
| `yerd tunnel share <SITE>` | Share a site via a Quick Tunnel (random `*.trycloudflare.com` URL). |
| `yerd tunnel stop <SITE>` | Stop sharing a site. |
| `yerd tunnel status` | Show live tunnels and `cloudflared` install status. |
| `yerd tunnel login` | Log in to a Cloudflare account (opens a browser) for named tunnels. |
| `yerd tunnel create <NAME>` | Create a named tunnel on the logged-in account. |
| `yerd tunnel delete <NAME>` | Delete a named tunnel and forget it locally. |
| `yerd tunnel list` | List the named tunnel, exposed sites, and authorized domain. |
| `yerd tunnel route <TUNNEL> <HOSTNAME>` | Create the proxied DNS record for a hostname. |
| `yerd tunnel set-host <SITE> [HOSTNAME] [--clear]` | Set or clear a site's public hostname (starts the named tunnel if needed). |
| `yerd tunnel password <SITE> [PASSWORD] [--clear]` | Set or clear a site's share password. |
| `yerd tunnel publish` | Start the named tunnel if it isn't running. Never restarts a live tunnel. |
| `yerd tunnel unpublish` | Stop the named tunnel until the next hostname change or daemon start. |

```sh
yerd tunnel install
yerd tunnel share app
yerd tunnel status
yerd tunnel stop app
```

## Quick tunnels

### `yerd tunnel install`

Downloads and installs `cloudflared`. Idempotent; safe to re-run. Needed once
before any share, unless a compatible `cloudflared` is already on `PATH` (see
above). Re-running it always switches Yerd back to the managed copy, even if a
`PATH` binary was previously in use.

### `yerd tunnel share <SITE>`

Publishes `<SITE>` (a site name like `app`, or its host `app.test`) at a random
`https://<name>.trycloudflare.com` URL and prints it, along with any [preflight
warnings](../../guide/sharing#preflight-warnings). It waits until Cloudflare's
edge reports the tunnel online; if it doesn't come online, the share is stopped
and the command fails with `could not start tunnel for <site>: <reason>`.
Requires `cloudflared` to be installed.

The URL is temporary - it changes on the next share and is torn down on `stop`
or daemon shutdown. Once online, a crashed quick tunnel is restarted
automatically, but with a **new** URL; quick tunnels are never resumed after a
daemon restart. Sharing a site that is already shared (even one that is
retrying) keeps the existing tunnel.

### `yerd tunnel stop <SITE>`

Tears down the site's tunnel. No-op if it isn't shared.

### `yerd tunnel status`

Lists live tunnels (site, state, URL/hostname) and whether `cloudflared` is
installed and logged in, plus its version and source: `managed` (Yerd's own
download) or `system` (found on your `PATH`) in `--json` output.

`STATE` is one of:

| State | Meaning |
| --- | --- |
| `connecting` | Starting; not connected to Cloudflare's edge yet. |
| `online` | Connected and serving. (`running` in `--json` output.) |
| `reconnecting` | Was online; the edge connection dropped and is recovering. |
| `failed` | The process exited or never connected. Printed as `failed, retrying`, since it is restarted after a backoff of at most 30 seconds. A quick share that fails to come up at all is removed and reported as an error instead. |

A password-protected share is marked `(password)`. Under each row, an
`error: …` line shows the most recent error from `cloudflared`, and `warning: …`
lines show the share's preflight warnings (on the named tunnel, each is prefixed
with the site it applies to).

```text
cloudflared: 2026.6.1

SITE	STATE	URL
app	online	https://calm-river-1234.trycloudflare.com	(password)
  warning: Vite dev server is running (the web root has a `hot` file), ...
```

## Named tunnels

Named tunnels serve a stable hostname on a domain you manage in Cloudflare. They
use one consolidated tunnel for every exposed site.

### `yerd tunnel login`

Opens a browser to authorize Yerd for a Cloudflare zone. The account certificate
is stored in the daemon's data directory. Run once per machine.

### `yerd tunnel create <NAME>`

Creates a named tunnel on the logged-in account and records it locally. Yerd
supports one named tunnel at a time; `delete` the existing one before creating a
differently-named tunnel.

### `yerd tunnel set-host <SITE> [HOSTNAME] [--clear]`

Maps `<SITE>` to a public `HOSTNAME` on your authorized domain and records the
mapping so the consolidated tunnel serves it. Create the DNS record for the
hostname with [`route`](#yerd-tunnel-route-tunnel-hostname) (the desktop app
does both in one step). `HOSTNAME` is **required unless** `--clear` is given -
`--clear` removes the site's mapping (and the two are mutually exclusive, so
forgetting the hostname can't silently clear it).

Setting a hostname starts the named tunnel in the background if it isn't
running (when you are logged in and have a tunnel); it never restarts a live
tunnel, so other exposed sites stay up. Clearing the **last** hostname stops the
named tunnel, and it stays off across daemon restarts until a hostname is set
again.

```sh
yerd tunnel set-host app app.example.com
yerd tunnel set-host app --clear
```

### `yerd tunnel publish` / `yerd tunnel unpublish`

`publish` starts the consolidated named tunnel if it isn't running, serving every
site that has a hostname set, and waits for it to come online. It is idempotent:
a live (or retrying) tunnel is left alone. If the tunnel can't connect, `publish`
still succeeds and the status shows `failed, retrying` with the error, because
the daemon keeps retrying. It fails up front only when something is missing:
`cloudflared` isn't installed, you aren't logged in, no tunnel has been created,
no site has a hostname, or the tunnel origin listener isn't running.

`unpublish` stops the named tunnel, taking all named sites offline, **until the
next hostname change or daemon start**. While any site has a hostname, setting
another hostname or restarting the daemon starts it again; clear the hostnames to
keep it offline.

### `yerd tunnel password <SITE> [PASSWORD] [--clear]`

Protects `<SITE>` with a share password. Tunnelled visitors (quick or named) are
asked for it through HTTP basic auth; any username works. Local `.test` access
is not affected. `PASSWORD` is **required unless** `--clear` is given, and
`--clear` removes the password (the two are mutually exclusive). Passwords are
1 to 128 bytes with no control characters, take effect on the next request, and
are stored in the `0600` config file. Protected sites are marked `(password)` in
`status` and `list`.

```sh
yerd tunnel password app 's3cret'
yerd tunnel password app --clear
```

::: tip Keep it out of your shell history
The password is a positional argument, so it ends up in your shell history.
Prefix the command with a space (with `HISTCONTROL=ignorespace` in bash, or
`setopt HIST_IGNORE_SPACE` in zsh), or set it from the desktop app instead.
:::

### `yerd tunnel route <TUNNEL> <HOSTNAME>`

Creates the proxied CNAME routing `HOSTNAME` to `TUNNEL` on your Cloudflare zone.
Run it once per hostname before (or after) `set-host`.

### `yerd tunnel list`

Shows the recorded named tunnel, the per-site hostname mappings that are exposed
(protected sites are marked `(password)`), and the **authorized domain**
resolved from your account.

### `yerd tunnel delete <NAME>`

Deletes the named tunnel from your Cloudflare account and forgets it locally:
stops the process, removes the credentials, and clears the exposed-site mappings.
Only the one configured tunnel may be deleted. The DNS records created by
`set-host`/`route` remain on your account - remove them in the Cloudflare
dashboard if you no longer want them.

::: tip Desktop app
The desktop app's **Share** page (under **Integrations**) drives all of this with
a searchable site picker, a per-site expose toggle, live tunnel health, and
per-site share passwords. See
the [Sharing Sites guide](../../guide/sharing).
:::
