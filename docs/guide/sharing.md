# Sharing Sites

Yerd can **publish a local site to the public internet** over a secure HTTPS URL,
so you can share work in progress, test a webhook, or demo from your own machine
without deploying anything. It works through [Cloudflare
Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/)
(`cloudflared`): the connection is **outbound-only** - no inbound ports, no
router config, no `sudo` - which fits Yerd's rootless model.

Sharing is opt-in and per-site. A site is never reachable from the internet until
you explicitly share it.

There are two tiers, and you can use either:

- **Quick share** - one click gives you a random `https://<name>.trycloudflare.com`
  URL. No Cloudflare account, no config, nothing to set up. Best for a quick
  demo or a one-off webhook test.
- **Named tunnels** - a stable hostname on **your own** Cloudflare-managed domain
  (e.g. `app.example.com`), backed by a tunnel on your account. Best when you
  want a URL that doesn't change.

::: warning Anyone with the URL can reach your site
Sharing serves your local site as-is. While a tunnel is up, anyone who has the
URL can load it unless you [set a share password](#password-protection). Stop
the tunnel when you're done. Quick-tunnel URLs are unguessable but public; named
hostnames are whatever you choose.
:::

## Prerequisite: cloudflared

Sharing needs the `cloudflared` binary. If one is already on your `PATH` (version
2023.3.0 or newer), Yerd detects and uses it automatically - nothing to install.
Otherwise install Yerd's own copy: `cloudflared` is a managed
[tool](./tooling), so Yerd **downloads it on demand** (the official Apache-2.0
static build, verified by checksum) into `{data}/tools/cloudflared/` - it isn't
bundled, and it never touches your system unless you already had one. No
Cloudflare account is required for quick share.

```sh
yerd install tool cloudflared
```

Once installed, `cloudflared` is also available as a command in `{data}/bin`, so
it is on your `PATH` after `yerd path install`. Yerd's copy is used ahead of one
on your `PATH`. Updates show up in **Tooling** like any other tool (re-run
`yerd install tool cloudflared` to update from the CLI), and
`yerd uninstall tool cloudflared` removes it; uninstalling is refused while a
tunnel is running and keeps your Cloudflare login and tunnels.

In the desktop app, install, update and uninstall `cloudflared` from
**Tooling**. The **Share** page links there the first time you visit, and its
**Setup** dialog shows which `cloudflared` is in use (**Found on PATH** or
**Yerd's copy**) with an **Open Tooling** link when it is missing or an update
is available.

<ThemedImage light="/images/share-setup-light.png" dark="/images/share-setup-dark.png" alt="The Share setup dialog: cloudflared installed, the Cloudflare account connected and the domain tunnel live with two sites" />

## Quick share

The fastest way to put a site online:

```sh
yerd tunnel share app        # -> https://calm-river-1234.trycloudflare.com
yerd tunnel status           # see live tunnels and their URLs
yerd tunnel stop app         # take it back offline
```

`share` waits until Cloudflare's edge reports the tunnel online and then prints
the URL. If it can't come online (for example, no network), the share is stopped
and the command fails with the reason, so a share reported as failed never goes
live later.

Quick tunnels are **ephemeral**:

- The URL changes every time you start one, and they're torn down when you stop
  them or the daemon exits. They are never resumed when the daemon restarts.
- Once online, a quick tunnel whose `cloudflared` crashes is restarted
  automatically, but Cloudflare hands out a **new URL** on every restart. Check
  `yerd tunnel status` (or the Share page) for the current one.
- They carry Cloudflare's quick-tunnel limits (around 200 concurrent requests,
  and no server-sent events), so they're for development, not production
  traffic.

::: tip "Server IP address could not be found"?
That's your machine's DNS resolver, not Yerd - some ISP resolvers don't resolve
fresh `*.trycloudflare.com` names. Point your system DNS at `1.1.1.1` or
`8.8.8.8` and the URL resolves immediately.
:::

## Named tunnels

Named tunnels publish a site at a **stable hostname on a domain you already manage
in Cloudflare**. They need a one-time browser login to your Cloudflare account.

Yerd uses a single consolidated tunnel that serves every site you expose (one
process, one config), so you create the tunnel once and then just toggle which
sites are live.

### 1. Connect your Cloudflare account

```sh
yerd tunnel login
```

This opens your browser to authorize Yerd for one of your Cloudflare zones. The
account certificate is stored in the daemon's data directory and never leaves
your machine. After login, Yerd resolves and shows the **authorized domain**
(e.g. `example.com`) so you know which hostnames you can use.

### 2. Create a tunnel

```sh
yerd tunnel create my-tunnel
```

Yerd supports one named tunnel at a time; remove it before creating another.

### 3. Expose a site

Create the DNS record for a public hostname on your authorized domain, then map
a site to it:

```sh
yerd tunnel route my-tunnel app.example.com   # create the proxied DNS record
yerd tunnel set-host app app.example.com      # record the mapping, go live
yerd tunnel list                              # show the tunnel, exposed sites, and domain
```

Setting a hostname is all it takes: the daemon starts the named tunnel if it
isn't already running. Adding or removing a site never restarts the tunnel, so
sites that are already exposed stay up while you change the others.

To stop exposing a site:

```sh
yerd tunnel set-host app --clear           # remove the mapping
```

Clearing the **last** hostname stops the named tunnel.

### Staying online

The daemon supervises the named tunnel for as long as at least one site has a
hostname:

- **Auto-restart.** If `cloudflared` exits or loses its connection to
  Cloudflare, it is restarted with a backoff of at most 30 seconds, and it keeps
  retrying for as long as it takes (hours of downtime included). There is no
  give-up state.
- **Resume on start.** When the daemon starts, it brings the named tunnel back
  up if any site has a hostname (and you are logged in with a tunnel created).
  Any stray `cloudflared` left behind by a daemon that crashed is stopped first,
  so an old connector can't keep serving stale config.
- **Stopping.** `yerd tunnel unpublish` stops the tunnel until the next hostname
  change or daemon start. To keep the named tunnel
  offline across restarts, clear its hostnames (`set-host <site> --clear`).
  `yerd tunnel publish` starts it again (it never restarts a live tunnel).

### Tunnel states

`yerd tunnel status` and the Share page report real edge health, not just
whether a process exists:

| State | Meaning |
| --- | --- |
| `connecting` | `cloudflared` is starting and has not reached Cloudflare's edge yet. |
| `online` | Connected to the edge and serving requests. |
| `reconnecting` | It was online, but the edge connection dropped; `cloudflared` is recovering. |
| `failed` | The process exited or never connected. Shown as `failed, retrying` because it is restarted after the backoff. A quick share that fails to come up at all is removed and reported as an error instead. |

When something went wrong, the most recent error from `cloudflared` is shown
under the tunnel (as an `error:` line in the CLI).

## In the desktop app

The [desktop app](./desktop-app) has a **Share** page in the sidebar (also
reachable from the command palette). When any site is shared, the sidebar item
shows a count.

- The page lists your shares in two groups: **Quick links** and **Your domain**
  (named after your Cloudflare zone). Each row shows the site, a status dot and
  its public address. On a narrower window the list becomes a table (site,
  type, public URL and status) and a share's details open in a drawer.

  <ThemedImage light="/images/share-light.png" dark="/images/share-dark.png" alt="The Share page listing quick links and sites on your domain, with a password-protected quick link selected" />

- Selecting a share shows its details: the **Public URL** with **Copy** and
  **Open**, a **Type** / **Local site** / **Status** strip with the tunnel's state
  (Connecting, Running, Reconnecting, Failed, retrying) and its last error, any
  [preflight warnings](#preflight-warnings) in amber, and, for a quick link, a
  note on its limits. A quick link has **Stop sharing**; a domain share has
  **Remove from domain**, plus **Start tunnel** when sites are exposed but no
  tunnel is running at all (for example after `yerd tunnel unpublish`) or
  **Restart tunnel** when it failed. A failed tunnel already retries on its own;
  restarting it picks up a newly installed `cloudflared`. Other exposed sites are
  never interrupted.
- The key button sets or removes the share's [share
  password](#password-protection). Protected shares show a key icon in the list.
- **Share a site** opens one dialog for both kinds: pick a site, then **Quick
  link** or **Your domain**. For your domain the **Public hostname** is
  pre-filled with `{site}.{your-domain}` and must stay on that domain; until
  Yerd knows your authorized domain the field stays empty and **Share** stays
  off. Sharing creates the DNS record and the daemon brings the tunnel up.
  Removing the last site takes it back down.

  <ThemedImage light="/images/share-new-light.png" dark="/images/share-new-dark.png" alt="The Share a site dialog sharing portal on your domain, with the public hostname pre-filled" />

- **Setup** holds the rest: which `cloudflared` is in use (with a link to
  Tooling to install or update it), connecting your Cloudflare account, and
  creating, starting or removing the tunnel for your domain. Closing the connect dialog before you
  authorize in the browser cancels the login.
- Each site's `⋯` menu on the [Sites page](./sites) also has a **Share
  publicly…** shortcut that jumps straight to sharing that site, without going
  to the Share page first:

  <ThemedImage light="/images/share-site-light.png" dark="/images/share-site-dark.png" alt="The site inspector's ··· menu on the Sites page, with Share publicly… highlighted" />

## How requests reach your site

Both tunnel kinds connect to a dedicated, loopback-only listener inside the
daemon rather than to the ports that serve `.test` traffic, so changing port
redirects or [elevation](./elevation) while sharing doesn't break the tunnel.
That listener looks up the **public** hostname, finds the site it belongs to,
and serves it through the same pipeline as local requests (proxy rules, static
files, route rules, PHP). A public hostname that isn't mapped to a site gets a
`404`.

To your application, a tunnelled request looks like this:

- `HTTP_HOST` and `SERVER_NAME` are the **public** hostname (for example
  `app.example.com` or `calm-river-1234.trycloudflare.com`), never the `.test`
  name, so URLs your app generates point at the public address.
- `HTTPS=on` and `SERVER_PORT=443`, because visitors use HTTPS at Cloudflare's
  edge. There is no HTTP-to-HTTPS redirect, and frameworks don't need
  `TrustProxies` configured to get the host and scheme right.
- `REMOTE_ADDR` is `127.0.0.1` for every visitor.

::: danger Localhost-only tools become public
Because every tunnelled visitor appears to come from `127.0.0.1`, anything your
app only enables for local requests - Laravel Debugbar, Telescope, Clockwork,
Symfony's profiler, an IP-gated admin page - is visible to anyone with the
share URL. Disable those tools (or set a [share
password](#password-protection)) while a site is shared.
:::

### WordPress

WordPress builds links from its stored `home` and `siteurl` options, so a site
installed as `https://app.test` would normally redirect public visitors back to
`.test`. For tunnelled requests to a WordPress site, Yerd injects a small
`auto_prepend_file` script that swaps the host in `home` and `siteurl` for the
public one (keeping each URL's path) for that request only. Local `.test` requests are unaffected, and nothing
in the site's files is changed.

Limits of the override:

- **Nothing is written to the database.** Saving **Settings > General** over the
  tunnel keeps the stored local URL, so the site still works on `.test`
  afterwards. Absolute URLs already stored in post content (images, links) are
  not rewritten and still point at `.test`.
- **`auto_prepend_file` interactions.** The override is passed to PHP-FPM as a
  per-request `PHP_VALUE`, which FPM keeps on the worker that served it. Until
  that worker is recycled, it can shadow an `auto_prepend_file` you set in your
  own `php.ini` for later requests on the same worker (the script itself does
  nothing on non-tunnel requests). A site-level `.user.ini` `auto_prepend_file`
  (for example, from a security plugin) takes precedence instead, in which case
  the public-URL override doesn't apply. The [one-click WordPress
  login](./sites#wordpress-one-click-admin-login) uses the same mechanism and
  has the same property.

## Password protection

Any shared site can require a password. Visitors are prompted by their browser
(HTTP basic auth); **any username** works with the right password.

```sh
yerd tunnel password app 's3cret'   # protect app on every tunnel
yerd tunnel password app --clear    # remove it
```

- The password applies to the site on both quick and named tunnels, and takes
  effect on the next request, without restarting anything.
- Local `.test` access is never prompted.
- Yerd checks the password itself and strips the `Authorization` header before
  the request reaches PHP. While a share is protected, your app never receives
  an `Authorization` header from tunnelled requests, so its own Basic or bearer
  token auth (an API, say) won't work over that share.
- It is stored in plain text in the `0600` [config file](../reference/configuration#tunnel)
  and is never sent back to the CLI or desktop app. Treat it as a lightweight
  gate for a dev share, not as production authentication. There is no rate
  limiting of attempts.
- Responses served behind the password are kept out of Cloudflare's shared
  cache: Yerd marks them `Cache-Control: private` (dropping `public` and
  `s-maxage`) and removes `CDN-Cache-Control`-style headers, since the edge
  cache does not key on the password. Browsers still cache as your app or
  [response headers](../reference/cli/headers) say. Pages the edge cached
  while the share was open can still be served until they expire, so set the
  password before sharing anything your app marks as publicly cacheable.

## Preflight warnings

When a share starts (`share`, `set-host`, `publish`, or the daemon resuming the
named tunnel), Yerd checks the site for setups that break for public visitors
and shows a warning. Warnings never block the share.

- **Vite dev server running.** The web root has a `hot` file, so visitors'
  browsers try to load assets from `localhost` on their own machine. Stop
  `npm run dev` or run a production build before sharing.
- **`SESSION_DOMAIN` pinned to the local TLD.** If `.env` sets `SESSION_DOMAIN`
  to `.app.test` (or anything under your TLD), session cookies are rejected on
  the public hostname and logins fail. Unset it, or set it to `null`, while
  sharing.

Warnings are computed once, when the share starts. After fixing the cause, they
clear the next time the share starts (stop and share again, or set the hostname
again).

## Removing a named tunnel

```sh
yerd tunnel delete my-tunnel
```

This stops the tunnel, deletes it from your Cloudflare account, removes its local
credentials, and clears the exposed-site mappings, leaving a clean slate. The
**DNS records** you created with `set-host`/`route` are *not* removed - delete
those in the Cloudflare dashboard if you no longer want them.

## How it stays safe

- `cloudflared` runs **unprivileged**, as the daemon user. Sharing never invokes
  the privileged helper and never opens a public listening port - the tunnel
  dials out, and the origin listener it connects to is bound to `127.0.0.1`
  only.
- The account certificate and per-tunnel credentials live in a daemon-owned `0700`
  directory; the desktop app never reads them.
- Nothing is shared until you ask, and quick tunnels are torn down on daemon
  shutdown.

## Command reference

See the [`yerd tunnel` CLI reference](../reference/cli/tunnel) for every command
and flag. The persisted mappings live in the `[tunnel]` section of the [config
file](../reference/configuration#tunnel).
