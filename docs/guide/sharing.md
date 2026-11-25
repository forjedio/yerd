# Sharing Sites

Yerd can **publish a local site to the public internet** over a secure HTTPS URL,
so you can share work in progress, test a webhook, or demo from your own machine
without deploying anything. It works through a tunnel provider:
[Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/)
(`cloudflared`), [ngrok](https://ngrok.com) or [zrok](https://zrok.io). Every
connection is **outbound-only** - no inbound ports, no router config, no `sudo` -
which fits Yerd's rootless model.

Sharing is opt-in and per-site. A site is never reachable from the internet until
you explicitly share it. A site can be shared through **several providers at
once**: each share gets its own public URL, and stopping one leaves the others
running.

Cloudflare has two tiers, and you can use either:

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

## Before you share

Each provider runs its own agent binary, and a share needs only the one for the
provider it goes through:

| Provider | Binary | Account needed |
| --- | --- | --- |
| Cloudflare | `cloudflared` (2023.3.0 or newer) | None for a quick link; a Cloudflare account for [your own domain](#named-tunnels) |
| ngrok | `ngrok` (3.16.0 or newer for a static domain) | An ngrok authtoken |
| zrok | `zrok2` (zrok v2) | A zrok account token, used once to enable this machine |

All three are managed [tools](./tooling): Yerd **downloads them on demand**
(official builds, verified by checksum) into `{data}/tools/` - none is bundled,
and nothing touches your system. A copy already on your `PATH` is detected and
used instead; once you install Yerd's own copy, that one is used ahead of it.

```sh
yerd install tool cloudflared
yerd install tool ngrok
yerd install tool zrok
```

Installed tools are also commands in `{data}/bin`, so they are on your `PATH`
after `yerd path install`. Updates show up in **Tooling** like any other tool
(re-run `yerd install tool <name>` to update from the CLI). Uninstalling a tool
is refused while a share through it is running.

In the desktop app, install, update and uninstall them from **Tooling**. The
**Share** page's [Providers](#tunnel-providers) sheet shows which binary each
provider uses (**Yerd's copy** or **Found on PATH**), with **Open Tooling**
when it is missing and **Update** when an update is available.

## Tunnel providers

Yerd shares through three tunnel providers: Cloudflare,
[ngrok](https://ngrok.com) and [zrok](https://zrok.io). Each keeps its own
account and its own URLs. On the **Share** page, **Providers** opens the
**Tunnel providers** sheet: the providers are listed on the left with a status,
the selected provider's settings are on the right, and **Default for new
shares** sits underneath the list.

| Status | Meaning |
| --- | --- |
| **Ready** | The provider can share. |
| **Needs setup** | The binary is there, but the account step is not done yet (ngrok: no authtoken saved; zrok: this machine is not enabled). |
| **Not installed** | No binary was found. Install it from **Tooling** (or `yerd install tool ngrok` / `yerd install tool zrok`). |

Each pane starts with the binary in use (see [Before you share](#before-you-share))
and a **Re-check** for after you install one.

From the CLI, `yerd tunnel providers` prints the same overview.

### Cloudflare

The Cloudflare pane shows the `cloudflared` binary, connects your Cloudflare
account, and creates, restarts or removes the tunnel for your domain (see
[Named tunnels](#named-tunnels)). Quick links need
no account, so Cloudflare is ready as soon as `cloudflared` is installed.

Uninstalling `cloudflared` keeps your Cloudflare login and tunnels.

<ThemedImage light="/images/share-setup-light.png" dark="/images/share-setup-dark.png" alt="The Cloudflare pane of the Providers sheet: Yerd's cloudflared installed, the Cloudflare account connected and the domain tunnel live with two sites" />

### ngrok

1. Install ngrok from **Tooling**.
2. Copy your authtoken from [dashboard.ngrok.com](https://dashboard.ngrok.com)
   and paste it into the **Authtoken** field, then **Save**.
3. Optionally enter your **Static domain** (free plans include one), such as
   `name.ngrok-free.app`. A pasted `https://` and trailing `/` are stripped.
   Leave it empty to use the address ngrok assigns to your account (on free
   plans, a fixed dev domain that doesn't change between shares).

The authtoken is stored in Yerd's [config file](../reference/configuration#tunnel)
(which only your user can read) and is handed to ngrok as `NGROK_AUTHTOKEN`. It
is never written to your shell profile or to ngrok's own config, and Yerd only
ever shows it masked (`2fXk••••••••Qa`). **Replace** swaps it, **Remove token**
deletes it. ngrok checks the token when a share starts, so a mistyped token
still reads as **Ready** until then.

<ThemedImage light="/images/share-providers-ngrok-light.png" dark="/images/share-providers-ngrok-dark.png" alt="The ngrok pane of the Providers sheet: Yerd's ngrok 3.39.13, a saved authtoken shown masked and the static domain acme-demo.ngrok-free.app" />

```sh
pbpaste | yerd tunnel ngrok token          # read the token from stdin
yerd tunnel ngrok domain name.ngrok-free.app
yerd tunnel ngrok token --clear
```

### zrok

zrok is enabled once per machine with a zrok account token:

1. Install zrok from **Tooling**.
2. Copy your account token from [zrok.io](https://zrok.io), paste it into
   **Enable this machine** and click **Enable**.

Yerd runs `zrok2 enable` in **its own zrok environment** under the data
directory (`{data}/tunnel/zrok`), so your own `~/.zrok2` is never touched, and
a zrok you enabled yourself does not affect Yerd. zrok keeps the account token
in that environment; Yerd does not store a second copy. While `zrok2 enable`
runs (a few seconds), the token is passed on its command line, so other users
on the same machine could briefly see it in the process list.

**Disable** runs `zrok2 disable`, releasing this machine from your zrok account,
so you can enable it again with any account token. If zrok refuses the disable
(for example because the account token was revoked), the local environment is
kept and the error is shown.

<ThemedImage light="/images/share-providers-zrok-light.png" dark="/images/share-providers-zrok-dark.png" alt="The zrok pane of the Providers sheet: Yerd's zrok2 2.0.11 with this machine enabled and a Disable button" />

```sh
pbpaste | yerd tunnel zrok enable
yerd tunnel zrok disable
```

### Default provider

**Default for new shares** (or `yerd tunnel default <cloudflare|ngrok|zrok>`)
picks the provider a new share uses unless you choose another. It starts as
Cloudflare.

## Quick share

The fastest way to put a site online:

```sh
yerd tunnel share app        # -> https://calm-river-1234.trycloudflare.com
yerd tunnel status           # see live shares, their provider and URLs
yerd tunnel stop app         # take it back offline
```

`share` uses the [default provider](#default-provider) (Cloudflare unless you
changed it); `--provider cloudflare|ngrok|zrok` picks another one. A provider
that isn't ready is an error that says what to set up; Yerd never falls back to
a different provider. `stop` without `--provider` stops every share of the
site (its own-domain mapping is left alone); with `--provider` it stops only
that one.

`share` waits until the provider reports the share online and then prints
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

## Share through ngrok

```sh
yerd tunnel share app --provider ngrok                 # your static domain, if saved
yerd tunnel share blog --provider ngrok --random-url   # your account's ngrok address
yerd tunnel stop app --provider ngrok
```

- Install ngrok from **Tooling** (or `yerd install tool ngrok`). A copy on your
  `PATH` works too, but it must be **ngrok 3.16.0 or newer** to use a static
  domain (older agents don't know the `--url` flag).
- Save your authtoken in the **Providers** sheet (or `yerd tunnel ngrok token`).
- With a [static domain](#ngrok) saved, shares use it unless you choose the
  **Account URL** (`--random-url`). Only one share can hold the static domain
  at a time; the desktop app disables it while another site's share uses it,
  and from the CLI ngrok's own error tells you. `--random-url` only helps when
  your account's address differs from the saved static domain. If the saved
  domain **is** your account's dev domain (the usual case on the free plan),
  `--random-url` lands on the same domain and fails the same way, so share the
  second site through another provider or a different ngrok domain instead.
- Without a static domain, or with `--random-url`, the share uses the address
  ngrok assigns to your account. On free plans that is a fixed dev domain, so
  it doesn't change between shares; `--random-url` only means "don't use
  Yerd's saved static domain", not "give me a new address".
- Free plans limit how many agents can run at once, so a second ngrok share may
  fail with ngrok's own error (for example `ERR_NGROK_108`). Yerd shows that
  error as-is.
- ngrok's free plan shows browsers a one-time warning page before your site;
  webhooks and API clients aren't affected.
- ngrok's local web inspector (port 4040) is turned off for Yerd's shares, so
  several agents never compete for it.

## Share through zrok

```sh
yerd tunnel share app --provider zrok
yerd tunnel stop app --provider zrok
```

- Install zrok from **Tooling** and [enable this machine](#zrok) first. Until
  then a zrok share is refused with a pointer to `yerd tunnel zrok enable`.
- Each zrok share is a public share with a **new URL every time it starts**.
- Stopping a share deletes it from your zrok account. A share that had to be
  killed hard (it ignored the stop signal for five seconds) can linger in the
  zrok console until zrok expires it.
- Disabling zrok is refused while a zrok share is running, and uninstalling
  ngrok or zrok is refused while a share through it is running.

Shares through ngrok and zrok are torn down when the daemon exits and are not
resumed when it starts again. A changed [outbound proxy](./corporate-networks)
applies to shares started after the change; restart a share to pick it up.

## Cloudflare named tunnels (your own domain) {#named-tunnels}

Named tunnels publish a site at a **stable hostname on a domain you already manage
in Cloudflare**. They need a one-time browser login to your Cloudflare account.

::: tip Cloudflare only
Named tunnels exist only for Cloudflare. ngrok's closest equivalent is a
[static domain](#ngrok): every ngrok share uses that same address, but it is on
ngrok's domain (such as `name.ngrok-free.app`), not yours, and only one site can
hold it at a time. zrok shares get a new URL each time they start.
:::

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

- **Providers** cards at the top show each provider's status (Cloudflare
  **Connected**, ngrok **Token saved**, zrok **Enabled**, or **Not installed** /
  **Not set up**); **Manage** opens the Providers sheet.
- Below them, **Shares** lists every share with the provider's icon, the site,
  the provider and kind (Cloudflare **Quick link** or **Your domain**, ngrok
  **Reserved domain** or **Account URL**, zrok **Public share**), a status dot
  and its public address. The **All / Cloudflare / ngrok / zrok** filter narrows
  the list. On a narrower window the list becomes a table (site, provider,
  public URL and status) and a share's details open in a drawer.

  <ThemedImage light="/images/share-light.png" dark="/images/share-dark.png" alt="The Share page with Cloudflare, ngrok and zrok provider cards, the All / Cloudflare / ngrok / zrok filter, and a password-protected Cloudflare quick link selected whose site is also shared through ngrok" />

- Selecting a share shows its details: the provider badge, the **Public URL**
  with **Copy** and **Open**, a **Provider** / **Local site** / **Status** strip
  with the share's state (Connecting, Running, Reconnecting, Failed, retrying),
  its last error and the binary in use, the site's other shares under **is also
  shared** (each with **Open**) plus **Also share through** for the providers it
  isn't shared through yet, any [preflight warnings](#preflight-warnings) in
  amber, and notes on a Cloudflare quick link's limits or ngrok's browser
  warning page. A per-site share has **Stop sharing**; a domain share has
  **Remove from domain**, plus **Start tunnel** when sites are exposed but no
  tunnel is running at all (for example after `yerd tunnel unpublish`) or
  **Restart tunnel** when it failed. A failed tunnel already retries on its own;
  restarting it picks up a newly installed `cloudflared`. Other exposed sites are
  never interrupted.
- The key button sets or removes the share's [share
  password](#password-protection). Protected shares show a key icon in the list.
- **Share a site** opens one dialog: pick a site, a **Provider** (the default is
  preselected; a provider that isn't ready offers **Set up**, which opens the
  Providers sheet on it), then the URL. For ngrok with a static domain saved,
  choose **Static domain** or **Account URL**. If the site is already shared
  through another provider, a note says the new share adds a second public URL.
  For Cloudflare choose **Quick link** or **Your domain**. For your domain the
  **Public hostname** is
  pre-filled with `{site}.{your-domain}` and must stay on that domain; until
  Yerd knows your authorized domain the field stays empty and **Share** stays
  off. Sharing creates the DNS record and the daemon brings the tunnel up.
  Removing the last site takes it back down.

  <ThemedImage light="/images/share-new-light.png" dark="/images/share-new-dark.png" alt="The Share a site dialog sharing portal through ngrok, with the Cloudflare, ngrok and zrok provider cards and the Static domain option selected" />

- **Providers** opens the [Tunnel providers](#tunnel-providers) sheet. Its
  Cloudflare pane holds the rest: which `cloudflared` is in use (with a link to
  Tooling to install or update it), connecting your Cloudflare account, and
  creating, starting or removing the tunnel for your domain. The ngrok and zrok
  panes set those providers up, and **Default for new shares** picks the
  default. Closing the connect dialog before you authorize in the browser
  cancels the login.
- Each site's `⋯` menu on the [Sites page](./sites) also has a **Share
  publicly…** shortcut that jumps straight to sharing that site, without going
  to the Share page first:

  <ThemedImage light="/images/share-site-light.png" dark="/images/share-site-dark.png" alt="The site inspector's ··· menu on the Sites page, with Share publicly… highlighted" />

## How requests reach your site

Every tunnel, whatever the provider, connects to a dedicated, loopback-only
listener inside the daemon rather than to the ports that serve `.test` traffic,
so changing port
redirects or [elevation](./elevation) while sharing doesn't break the tunnel.
That listener looks up the **public** hostname, finds the site it belongs to,
and serves it through the same pipeline as local requests (proxy rules, static
files, route rules, PHP). Cloudflare, ngrok and zrok all forward the public
hostname unchanged, so the same site and its [share
password](#password-protection) apply on every provider. A public hostname that
isn't mapped to a site gets a `404`.

To your application, a tunnelled request looks like this:

- `HTTP_HOST` and `SERVER_NAME` are the **public** hostname (for example
  `app.example.com` or `calm-river-1234.trycloudflare.com`), never the `.test`
  name, so URLs your app generates point at the public address.
- `HTTPS=on` and `SERVER_PORT=443`, because visitors use HTTPS at the
  provider's edge. There is no HTTP-to-HTTPS redirect, and frameworks don't need
  `TrustProxies` configured to get the host and scheme right.
- `REMOTE_ADDR` is `127.0.0.1` for every visitor.

::: danger Localhost-only tools become public
Because every tunnelled visitor appears to come from `127.0.0.1`, anything your
app only enables for local requests - Laravel Debugbar, Telescope, Clockwork,
Symfony's profiler, an IP-gated admin page - is visible to anyone with the
share URL. Disable those tools (or set a [share
password](#password-protection)) while a site is shared.
:::

### Laravel

A Laravel site mostly works over a share as-is: Yerd hands it the public host,
`HTTPS=on` and port 443, so `url()`, `route()`, redirects and `asset()` point at
the public address with no `TrustProxies` setup, and Livewire, Inertia and
ordinary forms keep working. A few things need attention first.

::: warning Build your Vite assets
Vite's dev server (`npm run dev`) runs on your machine at `localhost:5173`, which
visitors can't reach through a share, so pages load without their CSS and JS.
Stop the dev server and run `npm run build` before sharing, and build again
after changing front-end code. Yerd warns when a share starts if the dev server
is running (the web root has a `hot` file), or when a Laravel site has a Vite
config but no production build (`public/build/manifest.json` is missing). A
site whose pages don't use `@vite` (a fresh app's welcome page, say) can ignore
that second warning.
:::

::: warning Don't pin SESSION_DOMAIN to .test
If `.env` sets `SESSION_DOMAIN` to `.app.test`, browsers reject the session
cookie on the public hostname, so logins and CSRF-protected forms fail. Unset
it, or set it to `null`, while sharing. Yerd warns about this when a share
starts.
:::

::: info Things that read APP_URL still point at .test
Anything that builds a URL from `APP_URL` instead of the current request keeps
using the `.test` address: links in queued mail and notifications, scheduled
jobs and `artisan` commands, `Storage::url()` on the `public` disk, an
`ASSET_URL` you have set, and signed URLs generated in a queue. Set `APP_URL`
(and `ASSET_URL`, if used) to the share's URL while you test those, then put it
back.
:::

::: info Websockets and SPA auth need their own config
Laravel Reverb and other websocket servers aren't carried by the share: Echo
connects to the host built into your JS bundle (such as `VITE_REVERB_HOST`), so
real-time features only work locally. Sanctum's cookie-based SPA
authentication needs the share's hostname in `SANCTUM_STATEFUL_DOMAINS`.
:::

::: info Local-only tools become public
Debugbar, Telescope, Horizon's local gate, Clockwork and IP-restricted pages
treat every tunnelled visitor as `127.0.0.1` (see the warning above). Turn them
off or set a [share password](#password-protection) while the site is shared.
While a share is password-protected, your app's own Basic or bearer-token auth
won't receive its `Authorization` header.
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

- The password applies to the site on every share (quick, named, ngrok and
  zrok), and takes effect on the next request, without restarting anything.
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
and shows a warning: as `warning:` lines in the CLI's share and status output,
and in the desktop app as a notification when the share starts and in amber
on the share's details. Warnings never block the share.

- **Vite dev server running.** The web root has a `hot` file, so visitors'
  browsers try to load assets from `localhost` on their own machine. Stop
  `npm run dev` and run `npm run build` before sharing.
- **No Vite production build.** A Laravel site (it has an `artisan` file) with a
  `vite.config.*` but no `public/build/manifest.json`. Pages that load assets
  with `@vite` would be missing their CSS and JS, so run `npm run build` before
  sharing.
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

- Every tunnel agent runs **unprivileged**, as the daemon user. Sharing never invokes
  the privileged helper and never opens a public listening port - the tunnel
  dials out, and the origin listener it connects to is bound to `127.0.0.1`
  only.
- The account certificate and per-tunnel credentials live in a daemon-owned `0700`
  directory; the desktop app never reads them.
- Nothing is shared until you ask, and quick tunnels, ngrok and zrok shares are
  torn down on daemon shutdown.
- The ngrok authtoken is handed to ngrok only as `NGROK_AUTHTOKEN`, never on its
  command line, and Yerd runs ngrok with its own config file, so your
  `ngrok.yml` is never read. If ngrok echoes the token in an error, Yerd
  replaces it with `***` in the reported error and in the share's log when the
  share fails, crashes or stops. A share that keeps running after echoing it
  (say, after the token was revoked) keeps it in its log, which only your user
  can read, until the share stops.

## Command reference

See the [`yerd tunnel` CLI reference](../reference/cli/tunnel) for every command
and flag. The persisted mappings live in the `[tunnel]` section of the [config
file](../reference/configuration#tunnel).
