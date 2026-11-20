# Reverse Proxies

Yerd can put a `.test` address in front of a service it doesn't run itself - a
Reverb server, a Node or Vite dev server, a Docker container, anything already
listening on a port. The service gets Yerd's DNS, its trusted HTTPS, and a clean
`.test` hostname, with no extra config on the service's side.

There are two shapes, and you'll usually want the second for Laravel work:

- A **whole-host proxy** gives a service its own hostname: `reverb.test` → a
  service on `localhost:8080`.
- A **path rule** routes one path *on an existing site* to a service:
  `myapp.test/app` → a service, while every other path on `myapp.test` is still
  served by PHP. This keeps everything **same-origin**, which is exactly what
  Laravel Reverb needs (`wss://myapp.test/app`) so cookies and CORS work without
  a second domain.

Unlike [Herd](https://herd.laravel.com), which writes an nginx vhost, Yerd's
proxy is built into its own request path - there's nothing to configure and no
web server to reload.

## Whole-host proxies

Point a new `.test` host at a running service:

```sh
yerd proxy add reverb http://localhost:8080
```

Now `http://reverb.test/` reaches the service. Serve it over HTTPS the same way
you would a site - a proxy is secured on its own name:

```sh
yerd secure reverb
# https://reverb.test/  (trusted cert, HTTP redirects to HTTPS)
```

Point it somewhere else with `yerd proxy edit reverb http://localhost:8081`.
Its extra domains, wildcards and HTTPS setting stay as they are. Remove it with
`yerd proxy remove reverb`.

### Names and extra domains

A proxy's name may be **dotted**, so its own address can be a subdomain:

```sh
yerd proxy add api.account http://127.0.0.1:9011
# http://api.account.test/
```

Beyond that address, a whole-host proxy carries extra domains, subdomains, and
wildcards exactly as a site does - through the same [`yerd domain`](../reference/cli/domains)
commands, with the proxy name where a site name would go:

```sh
yerd proxy add account-dev http://127.0.0.1:48087
yerd domain add account-dev custom-domain.test
yerd domain add account-dev '*.account-dev.test'
yerd domain primary account-dev custom-domain.test
```

`yerd proxy list` shows a customised proxy's domains beneath it and marks the
primary; `yerd domain list` stays site-only. In the desktop app the same controls
are the **Domains** card in a proxy's inspector on the Proxies page.

<ThemedImage light="/images/proxies-light.png" dark="/images/proxies-dark.png" alt="The Proxies page: whole-domain proxies and paths on sites, with a proxy's route, HTTPS switch and domains in the inspector" />

A domain can only be claimed by one site or proxy. If two claim the same one,
every **site** is considered before any proxy, and a domain added explicitly
beats a claim on a default apex - a proxy that loses one domain keeps its
others, and [`yerd doctor`](../reference/cli/diagnostics) reports each contested
domain with its winner.

## Path rules (the Reverb case)

Attach a path to an existing site. Say `myapp` is a Laravel app and Reverb is
running on `:8080`:

```sh
yerd proxy add myapp /app http://127.0.0.1:8080
```

- `https://myapp.test/` and everything else → **PHP** (Laravel), unchanged.
- `https://myapp.test/app` (and `/app/...`) → **Reverb**, websockets included.

The rule inherits the site's TLS, so once `myapp` is secured the `/app` path is
too - your JS client connects to `wss://myapp.test/app` on the same origin. The
full path is passed through to the upstream unchanged (`/app/...` reaches
Reverb as `/app/...`), which is what Reverb expects.

Point a rule at a different upstream with
`yerd proxy edit myapp /app http://127.0.0.1:8081`; the site and path stay as
they are. Rules a service manages (such as the `/app` rule a site's Reverb
service adds) follow the service's port and can't be edited directly: change it
with `yerd service set-port reverb:myapp <port>` instead. Remove a rule with
`yerd proxy remove myapp /app`.

In the desktop app, **New proxy** on the Proxies page adds either shape: pick
**A whole domain** or **A path on a site**, then the name or the site and path,
and the upstream. To change an existing proxy's or path's upstream, use the
**Edit** button on its "Forward to" row in the inspector.

<ThemedImage light="/images/proxies-new-light.png" dark="/images/proxies-new-dark.png" alt="The New proxy dialog in A path on a site mode, forwarding invoicing.test/app to 127.0.0.1:8081" />

## Upstreams and headers

The upstream is `http://host:port` or `https://host:port`. In the desktop app
a missing scheme means `http://`, and a bare port (`3000`) means that port on
`127.0.0.1`. For an `https://` upstream, Yerd verifies the certificate for a genuine public host but **skips
verification for a local host** (`localhost`, a loopback/private IP, or a `.test`
name) - self-signed dev backends are the norm there.

Yerd preserves the original `Host` header (many upstreams key vhosts on it) and
adds `X-Forwarded-For`, `X-Forwarded-Proto`, `X-Forwarded-Host`, and `X-Real-IP`.
Websocket upgrades are tunnelled through. If the upstream is down, a request
returns **`502 Bad Gateway`** rather than hanging.

::: warning You can't proxy to Yerd itself
A target that points back into Yerd - a `.test` host, or `localhost` on the port
Yerd's own proxy is listening on - is rejected, because it would loop forever.
Point the target at the service's real port instead. In rootless mode Yerd binds
`8080`/`8443`, so a dev server on one of those ports will need to move.
:::

## Listing what's configured

```sh
yerd proxy list
```

shows every whole-host proxy and every per-site path rule. `yerd --json proxy
list` gives the same data as JSON for scripting or the desktop app.

For the full command surface, flags, and validation rules, see the
[Proxies CLI reference](../reference/cli/proxies).
