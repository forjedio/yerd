# Network

`yerd network proxy` controls the outbound HTTP proxy Yerd uses for its own
downloads, update checks, and the tools it runs. See the
[Outbound Proxy guide](/guide/corporate-networks) for how detection, the bypass
list and credentials work.

## Commands

| Command | Description |
| --- | --- |
| `yerd network proxy status` | Show the mode, the proxy in use (password redacted) and where it came from, and the bypass list. |
| `yerd network proxy set <URL> [--https <URL>] [--no-proxy <HOSTS>]` | Use a manual proxy. HTTPS uses the same proxy unless `--https` is given. `--no-proxy` takes a comma-separated list and replaces your bypass entries. |
| `yerd network proxy system` | Detect the proxy automatically (the default). Stored manual URLs are kept. |
| `yerd network proxy off` | Connect directly, ignoring proxy environment variables. Stored manual URLs are kept. |
| `yerd network proxy test` | Re-detect the proxy (including your login shell) and fetch `https://files.yerd.app/latest.json` through it. Exits `1` on failure. |

All commands accept `--json`, which prints the daemon's response as JSON.

## Examples

```sh
yerd network proxy set http://proxy.corp:3128 --no-proxy corp.com,10.0.0.0/8
yerd network proxy set http://alice:s3cret@proxy.corp:3128
yerd network proxy set http://127.0.0.1:3128      # a local px or cntlm
yerd network proxy status
yerd network proxy test
```

`URL` is `http://` or `https://`, with an optional `user:password@` and port.
The scheme defaults to `http` and the port to 80 or 443. Paths, queries and
SOCKS URLs are rejected.
