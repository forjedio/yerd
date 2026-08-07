# Routing rules

A **routing rule** says: URIs under a path prefix that match no real file are
handled by a target *inside the site*. It is the local-file counterpart to a
[proxy path rule](./proxies), which forwards to a separate running service.

Two shapes, one mechanism:

- A **nested front controller**: `/api` → `api/index.php`, for a legacy PHP
  portal that grew an API (a Yii, CodeIgniter, or Slim app mounted in a
  subdirectory). Requests like `POST /api/user/login` reach the nested app with
  their original URI intact, so its own router can handle them.
- A **JavaScript SPA**: `/` → `index.html`, so history-API deep links like
  `/dashboard/settings` serve the app shell instead of 404ing.

| Command | Description | Example |
| --- | --- | --- |
| `yerd route add <SITE> <PREFIX> <TARGET>` | Add a rule: URIs under `<PREFIX>` with no real file are handled by `<TARGET>`. | `yerd route add portal /api api/index.php` |
| `yerd route remove <SITE> <PREFIX>` | Remove a rule by its path prefix. | `yerd route remove portal /api` |
| `yerd route list [SITE]` | List routing rules, optionally for one site. | `yerd route list portal` |

```sh
# A legacy portal with a Yii API mounted at /api
yerd route add portal /api api/index.php
curl -X POST http://portal.test/api/user/login   # → api/index.php

# A Vite/Vue/React build: deep links serve the app shell
yerd route add dashboard / index.html
curl http://dashboard.test/settings/profile      # → index.html

yerd route list
yerd route remove portal /api
```

## How a rule is applied

Exactly nginx's `try_files $uri $uri/ <target>`:

1. A **real file** wins. `portal.test/api/openapi.json` serves that file if it
   exists on disk, rule or no rule.
2. A **real directory** still gets its trailing-slash redirect and its own
   `index.php` or `index.html`.
3. Only then does the rule apply, and the target handles the request.

The `<TARGET>` is a path relative to the site's web root, never an absolute
path and never containing `..`. It is checked against the real filesystem on
every request, so a symlink that resolves outside the site's document root is
refused rather than served or executed.

Prefix matching is boundary-correct and longest-prefix-wins: `/api` matches
`/api` and `/api/user` but not `/apix`, and a site holding both `/` and `/api`
sends `/api/user` to the `/api` rule. A prefix of `/` is a catch-all.

A `.php` target runs through PHP-FPM as a front controller and accepts **every**
HTTP method - that is what makes `POST /api/user/login` work. Any other target
is served as a static file and answers only `GET` and `HEAD`; anything else gets
`405 Method Not Allowed`.

::: warning A missing asset under a `/` rule returns 200, not 404
With a catch-all `/` → `index.html` rule, a request for a genuinely missing
`/assets/app.js` serves `index.html` with a `200`, so the browser reports a
JavaScript syntax error rather than a clean 404. This is deliberate, and is what
`try_files` and Vite's own preview server do: the rule cannot tell "a route the
SPA handles" from "an asset that should have been built" without guessing from
the file extension. If a script fails to parse, check that the asset was built.
:::

## Automatic SPA routing

A site whose web root holds an `index.html` and **no** `index.php` gets
history-API routing automatically, with no rule configured. Link a built
Vue/React/Svelte app and its deep links work immediately.

Adding an `index.php` turns this off, so a PHP app is never shadowed by a
stray `index.html`.

## Compared with the other routing commands

| | What it does |
| --- | --- |
| `yerd route` | Unmatched URIs under a prefix → **a file inside the site**. |
| [`yerd proxy`](./proxies) | A whole host or a path prefix → **a separate running service** over HTTP. |
| `yerd front-controller <site> on\|off` | Whether requests may execute a named `.php` directly, or must funnel through the site's single `index.php`. |

Routing rules work in both front-controller modes. When a site has both a proxy
path rule and a routing rule on the same prefix, the proxy rule wins: it
intercepts before PHP resolution runs at all.

## Where rules are stored

In `yerd.toml`'s `[route_rules]` table (schema v21+), keyed by site name for
linked sites and by document root for parked ones - the same split
[`[proxy_rules]`](./proxies) and `[domains]` use. Rules survive a site being
linked or unlinked, and are dropped when its parked root is un-parked. Changes
apply immediately; no restart is needed.
