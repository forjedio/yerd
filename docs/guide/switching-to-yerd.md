---
description: A step-by-step guide to switching to Yerd from Laravel Herd, Valet, or Lerd - stop the other tool's services first, elevate Yerd, verify, and how to switch back cleanly.
---

# Switching to Yerd

Yerd, [Laravel Herd](https://herd.laravel.com), [Valet](https://laravel.com/docs/valet), and [Lerd](https://github.com/geodro/lerd) all do the same OS-level job for local development, which means they all reach for the **same three system resources**:

- **Ports 80 and 443** - only one process can listen on each at a time.
- **The `*.test` resolver** - one OS resolver route per TLD (`/etc/resolver/test` on macOS, or systemd-resolved/NetworkManager snippets on Linux).
- **A trusted local CA** - each tool installs its own into the system trust store.

Because those are single-owner, **two tools can't run at once** without fighting over ports and DNS. Switching is therefore mostly about handing those three things over cleanly: stop the old tool, let Yerd take the ports/resolver, and (if you ever go back) hand them back.

::: tip The one rule
**Stop the other tool's services before you `sudo yerd elevate`.** Everything else is detail.
:::

This guide is for switching *machines you already use* for PHP dev. On a clean machine there's nothing to migrate - just follow [Getting Started](./getting-started).

## Before you start

- **Your project code is safe.** None of these tools own your source - they just serve folders. Switching changes *what's serving* `*.test`, not your files. You don't need to move or copy any project.
- **PHP versions don't carry over.** Yerd installs its own [prebuilt PHP builds](./php-versions); it doesn't reuse Herd/Valet/Homebrew PHP. [Getting Started](./getting-started) walks through installing the version(s) you need.
- **You can keep the old tool installed.** You don't have to uninstall Herd/Valet/Lerd to try Yerd - you just can't have both *serving* at the same time. Keeping it installed makes switching back trivial.

::: warning Subdomains are apex-only by default
If you relied on Valet/Herd resolving *any* subdomain of a site (`api.my-app.test`, `admin.my-app.test`, ...) implicitly, note that in Yerd a site answers only its exact apex `my-app.test` by default. Register the ones you need explicitly, or re-add the old catch-all behaviour with `yerd domain add my-app '*.my-app.test'`. See the [domains reference](../reference/cli/domains).
:::

## Step 1 - Stop the other tool

Stop whatever currently serves `*.test` so it releases ports 80/443 and stops answering `.test` DNS. Pick your tool:

### From Laravel Herd

Herd runs `nginx` + `dnsmasq` as background services.

- **Quit Herd** (or, in the app, pause/stop its services), **or** from the terminal:
  ```sh
  herd stop          # stop Herd's nginx/php/dnsmasq services
  ```
- Herd may relaunch its services on login. If you're switching for good, quit Herd fully (and consider removing it from Login Items) so it doesn't grab port 80 again on your next reboot.

### From Laravel Valet

Valet runs `nginx` + `dnsmasq` via Homebrew.

```sh
valet stop         # stop nginx + dnsmasq (releases 80/443 and *.test DNS)
```

To go further and have Valet remove its own DNS/loopback hooks too:

```sh
valet uninstall    # removes Valet's nginx/dnsmasq config (keeps your sites list)
```

`valet stop` is enough to switch; `valet uninstall` is for a permanent move.

### From Lerd

Lerd runs your stack in **rootless Podman containers**.

```sh
lerd stop          # stop Lerd's containers (frees 80/443)
```

If a stray container still holds a port, stop it directly with `podman stop <name>` (or `podman ps` to find it).

### Any other tool

The principle is the same: **stop whatever is listening on 80/443 and answering `*.test`.** To find a squatter on the ports:

```sh
# macOS / Linux - who holds 80 and 443?
sudo lsof -nP -iTCP:80 -sTCP:LISTEN
sudo lsof -nP -iTCP:443 -sTCP:LISTEN
```

## Step 2 - Install Yerd

With the other tool stopped, follow **[Getting Started](./getting-started)** to install Yerd and go through its first-run onboarding journey - it installs and starts the daemon, installs a PHP version, and parks a projects folder, all from the app.

You can do all of this **before** elevating: without elevation Yerd serves on the [rootless fallback ports](./elevation#the-rootless-fallback) `8080`/`8443`, so you can sanity-check it (`http://my-app.test:8080`) while the old tool is stopped.

## Step 3 - Elevate Yerd

Hand the three system hooks to Yerd. In the desktop app this is the **Doctor → Fix all** button; from the terminal:

```sh
sudo yerd elevate          # trust the CA · route *.test · allow 80/443
```

This trusts Yerd's local CA, points the `*.test` resolver at Yerd's DNS, and lets the daemon serve on 80/443 (a `pf` redirect on macOS, `setcap` on Linux). See [Elevation & Privileges](./elevation) for exactly what each step does.

::: tip macOS backs up your old resolver
On macOS, if a `/etc/resolver/test` already exists (a Valet/Herd leftover), `yerd elevate resolver` **saves a backup** before replacing it - so [switching back](#switching-back) can restore your previous DNS exactly. Linux has no backup mechanism; the drop-in is simply added/removed.
:::

## Step 4 - Verify

```sh
yerd doctor        # checks CA trust, the *.test resolver, ports, PHP, and sites
```

`doctor` is the source of truth for a clean switch. Watch for:

- **`ForeignWebListener`** - something *other than Yerd* is still holding 80 or 443. That's the old tool not fully stopped (go back to [Step 1](#step-1-stop-the-other-tool)). Yerd detects this by checking for its own `Server: yerd` marker on the port.
- **Ports fell back to 8080/8443** - elevation didn't take the privileged ports (often the same cause). Re-stop the other tool and re-run `sudo yerd elevate ports`.
- **Resolver / CA not configured** - re-run the matching `sudo yerd elevate <target>`.

Then open `https://my-app.test` - padlock and all.

::: warning Flush DNS / restart the browser
After the resolver changes, your OS or browser may still cache the old answer (or an HSTS/redirect from the previous tool). If a `.test` site won't resolve or shows a cert warning:

```sh
# macOS - flush the DNS cache
sudo dscacheutil -flushcache; sudo killall -HUP mDNSResponder
```

On Linux, Yerd reloads the detected supported resolver manager for you. Restart the browser if a site is stuck on the old CA or an HTTPS redirect.
:::

## Running side by side (without switching)

If you only want to *try* Yerd without giving up your current tool, **don't elevate**. Leave the other tool owning 80/443 and `*.test`, and reach Yerd's sites on its rootless ports instead:

- `http://my-app.test:8080` / `https://my-app.test:8443`

This avoids any conflict - but it's a trial mode, not the "just type the URL" experience. To make Yerd the default, stop the other tool and elevate.

## Switching back

Switching back is the reverse: hand the three hooks back to the other tool.

1. **Un-elevate Yerd** so it releases the resolver, CA trust, and ports:
   ```sh
   sudo yerd unelevate        # revert all three
   ```
   - On **macOS**, `unelevate resolver` **restores the resolver backup** taken in Step 3 (returning DNS to its pre-Yerd state), and `unelevate ports` removes the `pf` redirect.
   - On **Linux**, it removes all Yerd-owned systemd-resolved and NetworkManager resolver snippets. `setcap` has no clean reverse, so `unelevate ports` just prints the manual command (`sudo setcap -r <path-to-yerdd>`) - harmless to leave, but run it if you want it gone.

2. **Stop Yerd's daemon** so it isn't holding ports when the other tool starts:
   ```sh
   yerd stop          # or quit the desktop app (which stops the daemon)
   ```

3. **Start the other tool's services again** and let it re-claim the ports, DNS, and its own CA:
   ```sh
   valet start        # Valet  (or `valet install` if you uninstalled it)
   # herd start / launch Herd
   # lerd start
   ```

4. **Flush DNS** (see the warning above) and restart your browser.

::: tip Both CAs trusted is fine
Switching back doesn't *require* removing Yerd's CA - having more than one trusted local CA in your system store is harmless (the browser trusts certs from any of them). `sudo yerd unelevate trust` removes Yerd's if you want a tidy store; the helper only ever removes the cert whose subject is `Yerd Local CA`, never anyone else's root.
:::

## Removing Yerd entirely

If you're not coming back, do a full uninstall - which runs the same `unelevate` (all three targets) **before** deleting the binaries, so the system changes are reversed while `yerd-helper` still exists to reverse them:

```sh
sudo yerd uninstall        # un-elevates, then removes daemon, config, data, binaries
```

Run it **with `sudo`** - the trust/resolver/port changes can't be undone once the helper is gone. See the [Uninstall reference](../reference/cli/uninstall).

## See also

- [Getting Started](./getting-started) - install and first site
- [Elevation & Privileges](./elevation) - exactly what `elevate`/`unelevate` change
- [DNS & .test Domains](./dns) - how `*.test` resolution works
- [HTTPS & Certificates](./https) - the local CA and per-site certs
- [Diagnostics](./diagnostics) - `yerd doctor` and what it checks
