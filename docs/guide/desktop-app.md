---
description: A tour of the Yerd desktop app, screen by screen - the recommended way to install and run Yerd, a fast, rootless, open-source local PHP environment for macOS and Linux.
---

# Features

Yerd is a fast, rootless, open-source local PHP environment for macOS and Linux. It serves projects on `.test` domains over HTTP and HTTPS, runs a different PHP version per site, and manages it all from one small daemon. No Docker, no `sudo` for daily work, no subscription.

The **desktop app** is the recommended way to run all of it: a small tray-first window over everything the CLI does. Built with Tauri v2, Vue 3, TypeScript, and Tailwind, it's a thin client of the [daemon](./daemon), just like the `yerd` CLI - every button maps to one IPC request to `yerdd`, so the GUI and CLI can't drift out of sync. This page is a tour of everything it can do, screen by screen; each section links to the full guide for that feature.

If you live in the terminal, the [CLI](../reference/cli/) is a first-class alternative. Not installed yet? See [Getting Started](./getting-started).

## The window at a glance

The window controls sit at the top-left of the sidebar, above the Yerd logo and a status line showing whether the daemon is running (and its version). The sidebar opens on **Overview** and lists the pages in one flat list:

- **Overview** - a live dashboard of what's running
- **Sites** · **Proxies** · **PHP** · **Node** · **Tooling** · **Services** · **Mail** · **Dumps**
- **Share** - publish a site over a public URL ([guide](./sharing))
- Pinned at the bottom: **Doctor** · **Settings**

Some rows carry a badge: Sites shows your site count, PHP and Node their available updates, Mail its unread count (click it to open the Mail viewer), Share how many sites are shared, Dumps whether interception is on or off, Doctor an amber dot when a system privilege needs elevating, and Settings a badge when a Yerd update is available.

Each page opens with a toolbar: the page title, a search box that opens the [command palette](#keyboard-shortcuts), a **Docs** link, and the page's own actions. Drag the window by the toolbar or the top of the sidebar; double-click either to zoom.

### Overview

<ThemedImage light="/images/overview-light.png" dark="/images/overview-dark.png" alt="Overview dashboard" />

The landing dashboard, in two columns (they stack in a narrower window). On the left, **Your environment**: a status card showing whether Yerd is **serving**, how many `.test` sites it serves, the daemon's version, uptime, pid and running services, and health rows for the local CA, the `.test` resolver and the web ports (a problem row links to Doctor, or to Settings when the web ports can't be bound), with **Restart** and **Stop** buttons that ask before acting. Below it are your **favourite** sites (click one to open it in your browser), or a short setup checklist while nothing is installed or parked yet. On the right, the release notes for the version you're running, the two releases before it, and the people who contribute to Yerd. Release notes and contributors come through the daemon (and need a network connection the first time); while the daemon is down they show whatever was already loaded. When the daemon is down, the left column becomes a **Start Yerd** hero. While the daemon is running, this page polls it every 5 seconds, so a change made from the CLI or another window shows up without a manual refresh.

### Settings

<ThemedImage light="/images/settings-light.png" dark="/images/settings-dark.png" alt="Settings page" />

App- and daemon-level settings (one of the pages that stays usable when the daemon is down, since it can start or install it):

- **Daemon.** Whether `yerdd` is running (with pid), a Start or Stop button, and a list of the daemon's in-process subsystems - the DNS resolver, the HTTP and HTTPS proxy listeners (with bound ports, including when macOS's launchd port forwarder carries `:80`/`:443`), **Mail capture** (by port), and **Dump capture** (by port). The daemon row has a Restart button. Start/Stop/Restart go through your per-user service manager (systemd `--user` on Linux, a launchd LaunchAgent on macOS), with a detached-process fallback where none exists; the same actions are in the tray menu.
- **Application Ports** (while the daemon is running). Editable HTTP/HTTPS (the rootless fallback ports used when 80/443 need elevation), DNS, mail-capture, and dumps ports. If a port is in use elsewhere the page flags it here (site serving or `.test` resolution shows as unbound) so you can pick a free one. Change a value and **Save & restart** validates it, saves, restarts the daemon, and rechecks. HTTP/HTTPS are locked while ports are elevated - un-elevate them on the Doctor page first.
- **Start at login.** Three toggles - start the daemon at login, start the app at login, and start the app minimized (hidden to the tray). The daemon-at-login toggle is disabled where no per-user service manager is available.
- **Terminal CLI** (macOS and Linux). Installs `yerd` - and your installed tools (`php`, `composer`, ...) - onto your shell `PATH`. On a packaged Linux install `yerd` itself is already on `PATH`, so this is mainly how Linux users get the PHP/tool shims on `PATH` too.
- **Appearance.** A System / Light / Dark theme selector; a **Tray icon** selector (Automatic, Light Y, Dark Y, Full icon) for the menu bar / system tray icon; a **Title bar** selector (Automatic, macOS, Linux, Windows) that forces a window-control style regardless of host platform (the controls always sit on the left); and a **Preferred editor** selector (macOS and Linux) choosing which editor the Open-in-editor button uses - Auto-detect, any detected editor (VS Code, VSCodium, Cursor, Zed, Sublime Text, PhpStorm, Windsurf), or System default (open folder). Auto-detect picks the highest-ranked editor you have installed, preferring a dedicated PHP IDE over a general editor. **Rescan** re-runs detection if you install an editor while Yerd is open. All apply live and are remembered across launches.
- **About.** The app, daemon, and negotiated IPC protocol versions, links to the project site and repository, and the licence.
- **Updates.** A release-channel selector (Stable / Edge pre-releases), a **Check now** button, and the last-checked status (current version, latest stable/edge, and how long ago it checked). When an update is available, an **Apply update** button downloads, verifies, and installs it, restarting the app.
- **Troubleshooting.** **Logs** opens a dialog tailing the GUI's own session log (`yerd-gui.log`) alongside the daemon log, tab-switchable, with a copy button. **Diagnostics** gathers a shareable text snapshot of app/daemon state with its own copy button - useful when reporting a problem.

<ThemedImage light="/images/yerd-logs-light.png" dark="/images/yerd-logs-dark.png" alt="The Logs dialog, tailing the GUI session log" />

<ThemedImage light="/images/yerd-diagnostics-light.png" dark="/images/yerd-diagnostics-dark.png" alt="The Diagnostics dialog, a copyable JSON snapshot of app/daemon state" />

### PHP

<ThemedImage light="/images/php-light.png" dark="/images/php-dark.png" alt="PHP versions page" />

Manages your installed [PHP versions](./php-versions):

- A list of your versions, newest first, after a **Shared defaults** row: each shows a Default or Legacy chip, its patch and live FPM pool state, **Xdebug on** when its pool loads Xdebug, and an update chip or a **customised** tag. On a narrower window it becomes a table (patch, FPM, memory (RSS), Xdebug and update columns) and a version's details open in a drawer.
- **Check for updates** re-checks for patch updates; a banner under the list then offers **Update** or **Update all**. Updates are notify-only.
- **Install PHP** (or **Install another version** at the end of the list) opens one list of installable versions, supported first and legacy below, with installed ones marked. Picking a legacy version asks you to confirm you understand it's out of support. Installs download a prebuilt static build; the progress dialog can be closed and the install carries on in the background.
- A version's inspector shows its status, **Restart pool** (only when the pool is running or failed) and a `···` menu with Make default (not for legacy versions), Update (only when available) and Uninstall.
- Its **Settings** grid overrides the shared defaults for that version (empty fields inherit; changed rows get a blue dot and **Reset to shared** blanks them), plus **FPM workers** (`pm.max_children`, 1-1024, default 16, web pool only; the pool is on-demand, so a higher ceiling costs nothing while idle) and **FFI in web requests** when the build includes FFI. **Save changes** restarts that version's pool. See [Per-version configuration](./php-versions#per-version-configuration) and [Pool size](./php-versions#pool-size).
- **Extra extensions** registers extra `.so` extensions for the version (loaded into both FPM and the CLI): **Add extension** chooses the file, optionally names it and toggles Zend extension. Each is load-probed before saving, and any whose file has gone missing is flagged. **Other ini directives** edits free-form directives. See [Custom extensions](./php-versions#custom-extensions).
- The **Xdebug** section turns the [bundled Xdebug](./xdebug) on or off for the version (the switch applies immediately), and picks its modes and whether it starts on every request or only on a trigger (applied with **Apply**, which leaves the switch as it is). Turning the switch on while you have unapplied mode or start edits saves those edits with it, without pressing **Apply**; turning it off keeps them staged. The first enable can take a while as Yerd downloads and checks Xdebug; the daemon gives up after 90 seconds and changes nothing. While Xdebug is on, the **Other ini directives** card notes that the Xdebug section owns `xdebug.mode` and `xdebug.start_with_request` for the web pool and that `opcache.jit` is disabled. Versions without a published build show it as not available.
- The **Shared defaults** pane edits the global ini defaults applied to every version: `memory_limit`, `upload_max_filesize`, `post_max_size`, `max_execution_time`, `max_file_uploads`, `max_input_time`, `display_errors` and `error_reporting`. Leave a field blank to use PHP's built-in default. Its **FFI in web requests** switch sets `ffi.enable` for web requests (off by default; a hint appears when libvips isn't installed), and each version can override it when its build includes FFI. Saving restarts running pools to apply; **Restart all pools** restarts every running pool.

### Node

Manages your installed [Node.js versions](./node):

<ThemedImage light="/images/node-versions-light.png" dark="/images/node-versions-dark.png" alt="The Node page listing Node 26.13.1 (the default) and Node 24.22.3 with their LTS codenames" />

- **Refresh** and **Install** sit in the page toolbar. The Installed versions table shows each release with chips (a star on the global default, its LTS codename, and Current or EOL once Install or Refresh has loaded the release index), how many sites use it, and any pending update. Each row's `⋯` menu offers Update, Make default, and Uninstall. Pending updates show as a count on the sidebar's Node item and as an **Update Node** item in the tray menu. Uninstall is disabled while other releases exist if the global default would then resolve to nothing (for example an exact default naming that release); its confirmation lists any site preferences it would clear.
- Beside the table (below it on narrow windows), **How a site picks its Node** shows the resolution order and **Sites not on the default** lists the sites resolving to another release, with the source (`.nvmrc`, `.node-version` or saved) and a red chip for any whose release isn't installed.
- Install opens a dialog with a dropdown of the release lines not yet installed, newest first, with LTS codenames and EOL badges (end-of-life majors only when **Show end-of-life versions** is on); it installs the chosen line's newest patch, or switch to typing an exact version. Installs stream a live log; **Hide** closes it while they keep running.
- Update installs the newest patch alongside and moves the global default to it only when the default was on an older patch of that major. Site preferences are never changed.
- A banner notes that bare `node`, `npm`, and `npx` always use the global default; run `yerd exec npm …` inside a project to use its version.

### Sites

<ThemedImage light="/images/sites-light.png" dark="/images/sites-dark.png" alt="Sites page" />

The home base for [managing sites](./sites). Polls the daemon every 5 seconds while running, so sites added or changed elsewhere show up without a manual refresh.

The toolbar has **Park folder** (a native directory picker) and **New site**, which opens one two-step dialog, **Project** then **Options**, for Laravel, WordPress, Slim, CodeIgniter or CakePHP, or links an **Existing folder**. It runs one create at a time: closing the dialog leaves it running, and reopening **New site** shows its progress. See [Create a new site](./sites#create-a-new-site).

An amber strip under the toolbar flags busy or unforwarded web ports and an untrusted local CA; its action opens the **Doctor** page, where one click runs the fix. See [HTTPS & Certificates](./https).

The list is grouped: your [groups](./sites#site-groups), then each parked folder (a folder's header can reveal or un-park it), then **Linked**. Each row shows the `name.test` domain, its path, a `+N alias` count when it answers extra domains, a lock for HTTPS, and its PHP version (amber when it isn't the default). **Filter chips** narrow the list by HTTPS, linked, framework or "not on the default PHP", and the filter field matches names, paths and **any** domain a site answers - typing `admin.` finds `codestash.test` when its only match is an added `admin.codestash.test`. With the list focused, ↑/↓ select, Return opens the site, ⌘E / Ctrl+E opens your editor, ⌘T / Ctrl+T a terminal, and ⌘C / Ctrl+C copies the URL.

#### The site inspector

The selected site's details sit beside the list in a wide window, or slide in from the right when you click a row at the default size:

- **Actions.** **Open site**, a star that adds it to the Overview's favourites, the **Editor** button (macOS and Linux), **Terminal** (opens your terminal in the project directory) and **Finder** / **Files**, plus the site's URL with a copy button.
- **Environment.** PHP version; **Node** (**Default** or a release, a warning when a project file overrides the saved preference, an **Install Node X** button when the requested version isn't installed, and a tooltip reminding you to run `yerd exec node …` / `yerd exec npm …` in the project for the site's version); the **HTTPS** switch; **WordPress auto-login** with an admin picker on WordPress sites; the detected framework and how requests reach PHP; and whether the site is parked or linked.
- **Domains.** Every domain the site answers. Clicking one, or **Add domain**, opens a dialog to set the primary domain and add or remove aliases, subdomains, and wildcards. See [Domains](../reference/cli/domains).
- **Unpark site…** or, for a linked site, **Remove site…**, each with a confirmation. Your files are never deleted.

The **⋯** menu holds the rest: **Copy URL**; **Share publicly…** (a [Cloudflare Quick Tunnel](#share)); **Change web root…** (blank means auto-detect); **Move to group**, including **New group…**; **Routing rules…**, with the **Route through front controller** switch (on: everything funnels through `index.php`, as Laravel and Symfony want; off: named `.php` files execute directly) above the per-prefix [routing rules](../reference/cli/routes) that carve exceptions out of that default; **Response headers…**, custom [response headers](../reference/cli/headers) such as `Access-Control-Allow-Origin`; **Editor**, the editor used for this site; **WP Admin** or the **Dumps viewer**; and **Unpark** or **Remove site…**.

The **Editor** submenu defaults to the Preferred editor set in [Settings](#settings), shown as `Use default (…)`; pick a specific editor to override it for that one site, and the choice is remembered.

### Tooling

<ThemedImage light="/images/tooling-light.png" dark="/images/tooling-dark.png" alt="Tooling page" />

Installs self-contained developer tools - Composer, Bun, pnpm, the Laravel installer, and WP-CLI - onto your PATH alongside PHP, each managed by Yerd (install / update / uninstall) so they don't collide with system installs. Node.js isn't listed here: it has its own [Node](#node) page. See [Tooling](./tooling).

### Proxies

Puts a `.test` address in front of a service Yerd doesn't run itself. The list has two sections, matching the two shapes of [reverse proxy](./proxies), and **All / Domains / Paths** chips to filter them:

- **Domains** - whole-domain proxies, `grafana.test` → an upstream URL. Selecting one opens its inspector: **Open** and **Copy URL**, a **Route** card with the upstream and an **HTTPS** switch, and a **Domains** card listing the proxy's primary, wildcard and alias domains. Click any domain or **Add domain** to give the proxy extra domains, subdomains and wildcards exactly as a site gets them. **Remove proxy…** asks before it removes. Proxy names may be **dotted**, so a proxy's own address can be a subdomain (`api.account.test`).
- **Paths on sites** - one path on an existing site forwarded to a service (`myapp.test/app` → Reverb), keeping everything same-origin. Its inspector shows the upstream and **Remove path…**. A path's HTTPS follows its site.

**New proxy** opens one dialog with two modes. **A whole domain** takes the name (the `.test` suffix is added for you), the upstream (`http://` is assumed when you leave the scheme off) and whether to serve it over HTTPS. **A path on a site** takes the site, the path prefix (starting with `/`) and the upstream. An upstream can't be edited in place: remove the proxy and add it again.

In a narrow window the list becomes a table (forwards to, extra domains, TLS) and the inspector slides in as a drawer when you click a row. See [Reverse Proxies](./proxies).

### Services

<ThemedImage light="/images/services-light.png" dark="/images/services-dark.png" alt="Services page" />

The database and cache engines Yerd supervises - Redis (Valkey), MySQL, MariaDB, and PostgreSQL. Install a version, then Start / Stop / Restart it. Each installed engine's `⋯` menu also offers **Configuration** (copy the Laravel `.env` for that engine - with a database picker that pre-fills `DB_DATABASE` for SQL engines), Edit port, **Override settings**, View logs, **Manage databases** (create / drop / back up / restore, SQL engines only), Change version, and Uninstall. The daemon **auto-starts every installed engine** on boot. See [Services & Databases](./services).

**Override settings** opens a dialog listing the engine's stored [configuration overrides](./services#service-configuration-overrides) - free-form directives for its own config file, such as `max_allowed_packet` or `maxmemory-policy` - with an add form and a remove button per entry. Like Edit port, an override takes effect on the **next start or restart**, not immediately. The item only appears for the engines that have a config file to override; Meilisearch and Reverb are argv-driven, so they don't get it.

### Mail

<ThemedImage light="/images/mail-light.png" dark="/images/mail-dark.png" alt="Mail capture page" />

The built-in SMTP **mail capture** server - point your app's mailer at `127.0.0.1` on the shown port and every outgoing email is captured for preview instead of being sent. Toggle capture, set the port, and open the separate **Mails** viewer with Show Mails. A **Laravel configuration** card emits the `.env` mail keys (`MAIL_HOST`, `MAIL_PORT`, …) to paste into your app, with editable From name/address. Captured mail is tracked read/unread: the sidebar **Mail** item shows an unread-count pill (click it to jump straight to the viewer), and opening a message marks it read. See [Mail Capture](./mail).

### Dumps

<ThemedImage light="/images/dumps-light.png" dark="/images/dumps-dark.png" alt="Dumps page" />

Laravel telemetry interception - `dump()`/`dd()` plus queries, jobs, views, requests, logs, cache, and outgoing HTTP - streamed to a separate viewer window with no code changes, captured by a per-version PHP extension. Enable interception, pick which signals to record, set the port, and open the viewer with Show Dumps. See [Laravel ▸ Dumps](./laravel-dumps).

### Share

<ThemedImage light="/images/share-light.png" dark="/images/share-dark.png" alt="Share page" />

Publishes a local site to the public internet over Cloudflare Tunnel. A **Cloudflare Tunnel** card shows the detected `cloudflared` version; a **Shared sites** card picks a site and opens a Quick Tunnel with one click, alongside a live table of active tunnels showing each one's health (Connecting, Online, Reconnecting, Failed), its last error, and any preflight warnings. A separate **Named tunnels** card walks through connecting a Cloudflare account and exposing sites on your own domain. A key button on each shared site sets an optional share password. See [Sharing Sites](./sharing).

### Doctor

<ThemedImage light="/images/doctor-light.png" dark="/images/doctor-dark.png" alt="Doctor page" />

Mirrors [`yerd doctor`](./diagnostics):

- **Health.** Lists problems by severity (Healthy / Warning / Problem) with a copyable remedy command. Run safe fixes applies the safe one-click fixes; Re-check re-runs diagnostics. A clean machine shows an "all clear" panel.
- **Environment.** OS-level state: Local CA trusted, `.test` resolver installed, and Privileged ports (80/443). A Fix (elevate) button runs the privileged action where a row isn't configured; once a row *is* configured, an **Unelevate** button reverts it - behind an in-app confirm dialog and the OS prompt. Unelevating the `.test` resolver restores your previous resolver on macOS; reverting privileged ports is macOS-only (Linux `setcap` has no clean reverse, so no button is shown there).

::: info "Fix" actions never run the GUI as root
The Fix buttons run the audited `yerd elevate` helper under an OS prompt; the GUI never runs elevated. On Linux this uses `pkexec`, on macOS an `osascript … with administrator privileges` prompt. You may be asked for your password. See [Elevation & Privileges](./elevation).

On macOS, fixing Privileged ports from the app also removes any `pf` LAN redirect left by an older Yerd, without a message. If you use [LAN access](../reference/cli/lan), run `sudo yerd elevate lan` from a terminal afterwards.
:::

## Keyboard shortcuts

The window is fully keyboard-driven. Shortcuts follow each platform's convention: where macOS uses **Cmd** (`⌘`), Linux uses **Ctrl** with the same letter. Two of them are all you need to remember - the **command palette** (`⌘K` / `Ctrl+K`) jumps to any page or runs any action by typing, and the **shortcuts** overlay (`⌘/` / `Ctrl+/`) lists everything below in the app itself.

<ThemedImage light="/images/command-search-light.png" dark="/images/command-search-dark.png" alt="The command palette, listing Go to page actions" />

The command palette also lists your sites at the bottom (grouped by domain): **Open** a site in the browser, or **Secure / Unsecure** it (toggle HTTPS), without leaving the keyboard.

<ThemedImage light="/images/keyboard-shortcuts-light.png" dark="/images/keyboard-shortcuts-dark.png" alt="The keyboard shortcuts overlay" />

| Action | macOS | Linux | What it does |
|---|---|---|---|
| Command palette | `⌘K` | `Ctrl+K` | Search-and-run overlay for every page and action |
| Shortcuts | `⌘/` | `Ctrl+/` | Show this list inside the app |
| Go to a page | `⌘1` … `⌘9` | `Ctrl+1` … `Ctrl+9` | Jump straight to a sidebar page (see order below) |
| Settings | `⌘,` | `Ctrl+,` | Open the Settings page |
| Find | `⌘F` | `Ctrl+F` | Focus the page's filter box (Sites, Dumps) |
| New | `⌘N` | `Ctrl+N` | Start the page's primary action (Add site, Install PHP) |
| Refresh | `⌘R` | `Ctrl+R` | Re-fetch the current page's data |
| Restart daemon | `⇧⌘R` | `Ctrl+Shift+R` | Restart `yerdd` |
| Toggle theme | `⇧⌘L` | `Ctrl+Shift+L` | Switch light / dark (applies to every window) |
| Open Mail viewer | `⇧⌘M` | `Ctrl+Shift+M` | Open the standalone Mail capture window |
| Open Dumps viewer | `⇧⌘D` | `Ctrl+Shift+D` | Open the standalone Dumps telemetry window |
| Link Site | `⇧⌘N` | `Ctrl+Shift+N` | Open the Link-site dialog on the Sites page |
| Park Folder | `⇧⌘P` | `Ctrl+Shift+P` | Open the park-folder picker on the Sites page |
| Cycle Dumps tabs | `⌃⇥` / `⌃⇧⇥` | `Ctrl+Tab` / `Ctrl+Shift+Tab` | Move between categories in the Dumps viewer |
| Close window | `⌘W` | `Ctrl+W` | Hide the window to the tray |
| Close dialog | `Esc` | `Esc` | Dismiss the open modal |

`⌘1`…`⌘9` are: **1** Overview, **2** PHP, **3** Sites, **4** Tooling, **5** Services, **6** Mail, **7** Dumps, **8** Settings, **9** Doctor. Node, Proxies, and Share have no digit - reach them from the sidebar or the command palette.

::: info Quitting the app
There's no Quit shortcut: closing the window (`⌘W` / `Ctrl+W`) hides it to the tray and leaves the daemon running, by design. Quit from the tray menu, or on macOS with the standard `⌘Q`.
:::

## Related

- [Getting Started](./getting-started) - install Yerd and walk through the first-run onboarding journey
- [The Daemon](./daemon) - what `yerdd` is and how it runs
- [Sites](./sites) · [PHP Versions](./php-versions) · [HTTPS & Certificates](./https) - the features the GUI surfaces
- [Elevation & Privileges](./elevation) - how "Fix" actions stay root-free
- [CLI Reference](../reference/cli/) - the `yerd` command line, a first-class alternative
- [Desktop App Internals](../developer/gui) - the Tauri/Vue architecture for contributors
- [Source on GitHub](https://github.com/forjedio/yerd) - `apps/yerd-gui`
