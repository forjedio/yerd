# Response headers

A **response header** is a name and value Yerd adds to every response a site
serves. The usual reason is CORS: a frontend on `app.test` calling an API on
`api.test` needs the API to answer with `Access-Control-Allow-Origin`.

| Command | Description | Example |
| --- | --- | --- |
| `yerd header set <SITE> <NAME> <VALUE>` | Set a header on every response for the site. Setting a name the site already has replaces it. | `yerd header set api Access-Control-Allow-Origin https://app.test` |
| `yerd header remove <SITE> <NAME>` | Remove a header by name (case-insensitive). | `yerd header remove api Access-Control-Allow-Origin` |
| `yerd header list [SITE]` | List custom response headers, optionally for one site. | `yerd header list api` |

```sh
# A frontend on app.test calling a backend on api.test
yerd header set api Access-Control-Allow-Origin https://app.test
yerd header set api Access-Control-Allow-Credentials true
curl -sI http://api.test/ | grep -i access-control

yerd header list api
yerd header remove api access-control-allow-credentials
```

Values containing spaces need quoting in the shell:
`yerd header set api Access-Control-Allow-Headers "Content-Type, Authorization"`.

## How headers are applied

A configured header **replaces** any header of the same name already on the
response, so it appears exactly once with your value. It is applied to every
response Yerd produces for the site, on every domain the site answers and
through a public share tunnel:

- PHP responses, whatever their status (including the app's own `404`s and its
  answers to `OPTIONS` requests);
- static files and directory index pages;
- Yerd's own error pages for the site, such as the `500` shown when PHP is not
  reachable;
- Yerd's redirects, including the HTTP to HTTPS redirect for a secure site;
- WebSocket handshakes.

Two kinds of response are left untouched: those forwarded by a
[proxy path rule](./proxies) to a separate service, and those of a whole-host
proxy. Yerd also never answers a CORS preflight (`OPTIONS`) request itself;
your app answers it, and the configured headers are added to that answer.

::: warning Configure CORS in one place
Because a configured header replaces the app's own value, an app that already
sets the same header (for example Laravel's `HandleCors` middleware setting
`Access-Control-Allow-Origin`) has its value replaced. Pick either Yerd or the
app as the place CORS is configured, not both.
:::

## Validation

The name must be a valid HTTP header name, and the value must be non-empty with
no control characters (a tab is allowed); surrounding spaces are trimmed.
Headers Yerd manages itself are refused: `Connection`, `Keep-Alive`,
`Transfer-Encoding`, `Upgrade`, `TE`, `Trailer`, `Content-Length`, `Host`, and
any name starting with `Proxy-`. So are headers whose replacement would break
the response: `Set-Cookie` (it would drop every cookie the app sets),
`Content-Encoding` and `Content-Range` (they describe the body bytes), and
`Location` (it would hijack redirects).

## Where headers are stored

In `yerd.toml`'s `[response_headers]` table (schema v25+), keyed by site name
for linked sites and by document root for parked ones, the same split
[`yerd route`](./routes) uses. Headers survive a site being linked or unlinked,
and are dropped when its parked root is un-parked. Changes apply immediately; no
restart is needed.
