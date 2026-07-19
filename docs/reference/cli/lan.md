# LAN sharing

By default Yerd serves your `.test` sites only to the machine they run on -
every listener binds `127.0.0.1`. `yerd lan` opts into exposing them to **other
devices on your local network** (a phone, a tablet, another laptop) over the same
ports 80/443, and `yerd remote-setup` provisions a device so it trusts Yerd's CA
and resolves `.test`.

::: warning This exposes your dev sites to the network
LAN sharing binds the web proxy and DNS responder to `0.0.0.0`, so anything that
can route to your machine can reach your sites (subject to the peer filter and
your host firewall). It is opt-in and off by default. Turn it off with
`yerd lan disable` when you're done. Don't enable it on an untrusted network.
:::

## Commands

| Command | Description |
| --- | --- |
| `yerd lan enable` | Expose `.test` sites to the LAN, then restart the daemon to re-bind. |
| `yerd lan disable` | Return to loopback-only, then restart the daemon. |
| `yerd lan status` | Show configured-vs-effective state, the LAN IP, and the next step. |
| `yerd remote-setup` | Mint a one-time command to provision another device. |

## How it works

- **The web proxy and DNS responder bind `0.0.0.0`** while LAN mode is on. A
  peer filter drops any connection whose source address isn't private
  (RFC 1918 / link-local / loopback) - a blast-radius reducer, **not**
  authentication, so a host firewall is still recommended.
- **DNS answers split-horizon.** Your own machine keeps resolving `.test` to
  `127.0.0.1`; other LAN devices get your machine's LAN IP. IPv6 (`AAAA`) is not
  served in LAN mode.
- **Databases, mail capture, dumps, and the IPC socket stay loopback-only.** Only
  the web + DNS + the bootstrap endpoint leave loopback.
- Enabling or disabling **restarts the daemon** (a listen socket's bind address
  is fixed when it's opened), so the change is enforced, not merely saved.

## Privileged setup for ports 80/443

LAN devices expect the well-known ports. Binding them needs a one-time,
per-machine privileged step - the same mechanism as ordinary Yerd use:

- **macOS:** run `sudo yerd elevate ports` (if you haven't already) **and**
  `sudo yerd elevate lan`. The first installs the loopback redirect for on-host
  access; the second installs a `pf` redirect that carries inbound LAN 80/443 to
  Yerd on your LAN IP. Remove the LAN rule later with `sudo yerd unelevate lan`.
- **Linux:** run `sudo yerd elevate ports` once (it grants
  `cap_net_bind_service`, which covers the `0.0.0.0` bind). No separate LAN step.

`yerd lan status` tells you whether these are in place.

## Firewall

Yerd does not configure your firewall. Allow these from your LAN subnet only:

| Port | Protocol | Purpose |
| --- | --- | --- |
| 80, 443 | TCP | web proxy |
| 1053 | UDP + TCP | `.test` DNS responder (or your configured `dns_port`) |
| 7073 | TCP | remote-setup bootstrap (or your configured `lan_setup_port`) |

Example (`ufw`, replace the subnet):

```sh
sudo ufw allow from 192.168.1.0/24 to any port 80,443,7073 proto tcp
sudo ufw allow from 192.168.1.0/24 to any port 1053
```

::: info IPv6
Yerd binds only IPv4 (`0.0.0.0`) listeners, so there is nothing to reach over
IPv6 today. Note only that IPv4 firewall rules don't cover IPv6 in general.
:::

## Provisioning a device — `yerd remote-setup`

A remote device needs two things to use your `.test` sites: it must **trust
Yerd's CA** (for HTTPS) and **resolve `.test`** to your machine. `yerd
remote-setup` prints a command that does both. It only works while LAN mode is
up.

```sh
$ yerd remote-setup
Run this on the OTHER device (needs sudo, curl, and openssl):

  curl -fsS 'http://192.168.1.42:7073/remote-setup/ca?code=…' -o yerd-ca.pem \
    && test "$(openssl x509 -in yerd-ca.pem -noout -fingerprint -sha256 \
                | sed 's/.*=//;s/://g' | tr A-Z a-z)" = "<fingerprint>" \
    && curl -fsS --cacert yerd-ca.pem 'https://192.168.1.42:7073/remote-setup?code=…' -o yerd-setup.sh \
    && sudo bash yerd-setup.sh <fingerprint>
```

::: danger The fingerprint is the trust anchor - verify it
The command downloads Yerd's CA over plain HTTP, then **verifies its fingerprint
matches the one printed on your screen** before trusting it, and only then
fetches the installer over HTTPS validated against that CA. The fingerprint
travels by your eyes, not the wire - that's what makes this safe. Do not edit it
out. A wrong or missing fingerprint aborts the install. The code is single-use
and expires in 15 minutes.
:::

Supported devices: **macOS** and **Linux with dnsmasq or NetworkManager**. A
Linux box using **systemd-resolved alone** is not supported (it can't forward a
single domain to a custom port) - install dnsmasq or use NetworkManager.

### Undoing it on a device

Yerd can't revert a device it doesn't control. On each provisioned device, run
the installer's uninstall mode to remove the CA and the resolver entry:

```sh
sudo bash yerd-setup.sh <fingerprint> uninstall
```

## Headless / always-on hosts

If you share from a machine you don't stay logged into, enable a persistent user
session so `yerdd` keeps running (Linux):

```sh
sudo loginctl enable-linger "$(whoami)"
```

See the [daemon guide](./daemon) for details.

## Turning it off

```sh
yerd lan disable
```

This restarts the daemon back onto loopback. On macOS the `pf` LAN redirect is
separate privileged state - `yerd lan status` flags it as residual until you run
`sudo yerd unelevate lan`. A full `yerd uninstall` removes it too.
