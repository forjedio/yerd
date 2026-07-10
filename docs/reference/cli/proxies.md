# Proxies

A **reverse proxy** fronts an already-running service with a Yerd `.test`
address, so it gets the same DNS, HTTPS, and browser experience as a PHP site -
without Yerd running the service itself. Two shapes:

- A **whole-host proxy** maps a new host to a service: `reverb.test` → a Reverb,
  Node, Vite, or Docker service on some port.
- A **path rule** maps a path *on an existing site* to a service:
  `myapp.test/app` → a service, while every other path on `myapp.test` keeps
  being served by PHP. This is the same-origin setup Laravel Reverb wants
  (`wss://myapp.test/app`), so cookies and CORS just work.

`yerd proxy add`'s two forms are distinguished by argument count: two arguments
create a whole-host proxy, three attach a path rule to a site.

| Command | Description | Example |
| --- | --- | --- |
| `yerd proxy add <NAME> <URL>` | Create a whole-host proxy (`<NAME>.test` → `<URL>`). | `yerd proxy add reverb http://localhost:8080` |
| `yerd proxy add <SITE> <PREFIX> <URL>` | Add a path rule: requests under `<PREFIX>` on `<SITE>.test` proxy to `<URL>`. | `yerd proxy add myapp /app http://127.0.0.1:8080` |
| `yerd proxy remove <NAME>` | Remove a whole-host proxy. | `yerd proxy remove reverb` |
| `yerd proxy remove <SITE> <PREFIX>` | Remove a path rule from a site. | `yerd proxy remove myapp /app` |
| `yerd proxy list` | List every whole-host proxy and per-site path rule. | `yerd proxy list` |

```sh
# Whole-host: front a Reverb server on its own .test domain
yerd proxy add reverb http://localhost:8080
curl http://reverb.test/          # → the Reverb service

# Serve it over HTTPS - a proxy secures exactly like a site
yerd secure reverb
curl https://reverb.test/

# Path rule (the Reverb same-origin case): /app on an existing Laravel site
yerd proxy add myapp /app http://127.0.0.1:8080
# https://myapp.test/        → PHP (Laravel)
# https://myapp.test/app     → Reverb (websockets included)

# List, then remove
yerd proxy list
yerd proxy remove myapp /app
yerd proxy remove reverb
```

A path rule inherits its parent site's TLS: securing `myapp` (`yerd secure
myapp`) also secures its `/app` rule. A whole-host proxy is secured on its own
name (`yerd secure reverb` / `yerd unsecure reverb`), exactly like a site.

## Upstreams

The upstream `<URL>` is `http://host:port` or `https://host:port`. The port is
required for anything but the default (`80`/`443`). For an `https://` upstream,
Yerd verifies the certificate for a genuine public host, but **skips
verification for a local host** (`localhost`, a loopback/private IP, or a
`.test` name) - self-signed dev backends are the norm there.

The request path is passed to the upstream **unchanged** (an nginx
`proxy_pass` with no trailing path), so a path rule's prefix reaches the
upstream too: `myapp.test/app/foo` → `<URL>/app/foo`. Yerd preserves the
original `Host` header and adds the usual `X-Forwarded-For`,
`X-Forwarded-Proto`, `X-Forwarded-Host`, and `X-Real-IP` headers. Websocket
(`Connection: Upgrade`) traffic is tunnelled through, so Reverb/Vite HMR work.

If the upstream is down, a proxied request returns **`502 Bad Gateway`** rather
than hanging.

## How a request routes

For an incoming host, Yerd resolves it to a site or a whole-host proxy, then:

- a **whole-host proxy** forwards every request to its upstream (no PHP);
- a **site** with a matching **path rule** forwards that request to the rule's
  upstream; every other path is served by PHP as usual. Matching is
  longest-prefix with path boundaries, so `/app` matches `/app` and `/app/x`
  but **not** `/apple`.

If a whole-host proxy's name collides with a real site's apex, the site wins and
the proxy is dropped (surfaced by [`yerd doctor`](./diagnostics)).

::: details Client-side validation & guards
`proxy add` validates the upstream URL and, for a path rule, that the prefix
begins with `/`, before connecting - a malformed value fails with a usage error
(exit code `2`). The daemon rejects a target that would **loop back into Yerd**:
a `.test` host, or a loopback host on one of Yerd's own bound proxy ports. A
proxy name that collides with an existing site or proxy is rejected as
"already exists".
:::

## See also

- [HTTPS & Certificates](../../guide/https) - how `secure` mints a trusted cert.
- [Reverse Proxies](../../guide/proxies) - the guide-level walkthrough.
- [Domains](./domains) - give a proxy or site more `.test` names.
