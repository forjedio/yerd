# Outbound Proxy

On a corporate network, traffic to the internet often has to go through an HTTP
proxy. Yerd makes its own outbound requests - downloading PHP, Node, pnpm, Bun,
Composer, service engines and cloudflared, checking for updates, creating sites -
and it can send all of them through a proxy.

## Modes

| Mode | What Yerd does |
| --- | --- |
| **System** (default) | Detects the proxy automatically (see below). |
| **Manual** | Uses the proxy URLs you enter. |
| **Off** | Connects directly and ignores any proxy environment variables. |

Switch modes from **Settings ▸ General ▸ Network** in the desktop app, or with the
CLI:

```sh
yerd network proxy status                          # mode, proxy in use, where it came from
yerd network proxy set http://proxy.corp:3128      # manual proxy for HTTP and HTTPS
yerd network proxy set http://a:3128 --https http://b:3129 --no-proxy corp.com,10.0.0.0/8
yerd network proxy system                          # back to automatic detection
yerd network proxy off                             # direct connections only
yerd network proxy test                            # fetch Yerd's release manifest through the proxy
```

## How System mode detects the proxy

Yerd checks these sources in order and uses the first one that names a proxy:

1. **The daemon's own environment** - `http_proxy`, `https_proxy`, `all_proxy`
   and `no_proxy`, lowercase or uppercase (lowercase wins). `all_proxy` only
   fills in a scheme that isn't set otherwise.
2. **The operating system's settings** - macOS **System Settings ▸ Network ▸
   Proxies** (Web Proxy and Secure Web Proxy, plus the bypass list), or the GNOME
   proxy settings on Linux (`gsettings org.gnome.system.proxy`).
3. **Your login shell** - the `*_proxy` variables exported by your shell's
   startup files (`~/.zshrc`, `~/.bashrc`, `~/.config/fish/config.fish`, ...).

The source that won is shown in `yerd network proxy status` and in the Network
card, for example `http://proxy.corp:8080 (macOS System Settings)`.

The environment and OS settings are re-read at most once a minute, so a VPN that
switches the system proxy is picked up within about a minute. Your login shell is
only read when Yerd starts, when you save the network settings, and when you run
a connection test.

::: tip Changed your shell's proxy exports?
After editing the proxy variables in your shell's startup files, press **Test
connection** in the Network card (or run `yerd network proxy test`), or restart
Yerd, so it reads them again.
:::

KDE proxy settings are not read; set the proxy manually instead.

## What goes through the proxy

- Every download and update check the daemon makes: PHP (including legacy
  builds and extensions), Node, pnpm, Bun, Composer, service engines,
  cloudflared, the WordPress version list, Yerd's own self-update, and the
  Cloudflare API calls behind named tunnels.
- The tools the daemon runs while creating sites or installing tools: Composer,
  the Laravel installer, WP-CLI, npm, pnpm, Bun and git get `HTTP_PROXY`,
  `HTTPS_PROXY`, `NO_PROXY` (both cases) and `NODE_USE_ENV_PROXY=1`.
- The commands in Yerd's bin directory - `php`, `composer`, `laravel`, `wp`,
  `node`, `npm`, `npx`, `pnpm`, `pnpx`, `bun` and `bunx` - when you run them
  yourself. Yerd writes the resolved proxy to `network.env` in its data
  directory and these commands fill it in, **but only when your shell doesn't
  name a proxy of its own**. If any of `http_proxy`, `https_proxy` or
  `all_proxy` (either case) is set in your environment, Yerd adds nothing and
  your settings win. If you only set `no_proxy`, Yerd still adds its proxy and
  sets both `no_proxy` and `NO_PROXY` to your entries followed by its own
  (loopback and `.test`), so your bypass list is kept.

Your sites' own PHP code (requests made by the app you are developing) is not
changed.

## Bypass list

Some hosts are always reached directly: `localhost`, `127.0.0.1`,
`127.0.0.0/8`, `::1`, and every site on your Yerd TLD (`.test`). Your own bypass
entries are added to those, together with the entries from the detected source
(for example the macOS "Bypass proxy settings for these hosts" list).

Entries are normalised to a form curl, Node and Composer all understand:

| You enter | Yerd uses |
| --- | --- |
| `corp.com` | `corp.com` (the domain and every subdomain) |
| `*.corp.com` | `.corp.com` (the domain and every subdomain) |
| `.corp.com` | `.corp.com` (kept as given) |
| `intranet` | `intranet` |
| `169.254/16` | `169.254.0.0/16` |
| `host.corp:8080` | `host.corp` (ports are dropped) |

macOS's **Exclude simple hostnames** checkbox is not applied; add those names to
the bypass list if you need them.

## Credentials

Proxies that use **Basic** authentication work by putting the user name and
password in the URL:

```sh
yerd network proxy set http://alice:s3cret@proxy.corp:3128
```

Percent-encode special characters in the password (`@` as `%40`, `:` as `%3A`,
`/` as `%2F`). The URL is stored in `yerd.toml`, which Yerd writes readable only
by your user. Everywhere else - `status`, the desktop app, logs, error
messages - the password is shown as `***`.

When you edit the settings in the app, the password field keeps showing `***`;
saving without changing the host keeps the stored password. If you change the
scheme, user, host or port, re-enter the password.

Credentials from macOS System Settings (kept in the keychain) and GNOME are not
read. If your proxy needs a password, use Manual mode.

## PAC files, NTLM and Kerberos

Yerd does not evaluate proxy auto-config (PAC) scripts or WPAD, and it does not
support NTLM, Kerberos or Negotiate authentication. In those setups, run a small
local proxy that does, and point Yerd at it:

- [px](https://github.com/genotrance/px) handles PAC and NTLM/Kerberos and listens
  on `127.0.0.1:3128` by default.
- [cntlm](https://cntlm.sourceforge.net/) handles NTLM.

```sh
yerd network proxy set http://127.0.0.1:3128
```

When System mode finds a PAC script or WPAD but no fixed proxy, `yerd doctor`
reports `OutboundProxyPacUnsupported`.

SOCKS proxies are not supported.

## Corporate certificates

Many corporate proxies inspect HTTPS with their own root certificate. Yerd trusts
the certificates your operating system trusts, so once IT has installed that root
in the system store, downloads through the proxy work without extra setup.

## Cloudflare tunnels

Sharing a site with `yerd tunnel` uses cloudflared, whose connection to
Cloudflare's edge never goes through an HTTP proxy, so tunnels need direct
outbound access. cloudflared's management commands (`tunnel login`, `tunnel
create`, `tunnel route`) don't receive the proxy settings yet either. Downloading
cloudflared and the Cloudflare API lookups Yerd makes itself do use the proxy.

## Troubleshooting

- `yerd network proxy test` re-detects the proxy and fetches Yerd's release
  manifest through it, printing the HTTP status and the route it took. It exits
  with status 1 on failure.
- `yerd doctor` warns with `OutboundProxyUnreachable` when the proxy in use
  doesn't accept connections.
- `yerd network proxy off` turns the feature off immediately if a detected proxy
  is wrong.
