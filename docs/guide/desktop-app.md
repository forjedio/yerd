---
description: A tour of the Yerd desktop app, screen by screen - the recommended way to install and run Yerd, a fast, rootless, open-source local PHP environment for macOS and Linux.
---

# Features

Yerd is a fast, rootless, open-source local PHP environment for macOS and Linux. It serves projects on `.test` domains over HTTP and HTTPS, runs a different PHP version per site, and manages it all from one small daemon. No Docker, no `sudo` for daily work, no subscription.

The **desktop app** is the recommended way to run all of it: a small tray-first window over everything the CLI does. Built with Tauri v2, Vue 3, TypeScript, and Tailwind, it's a thin client of the [daemon](./daemon), just like the `yerd` CLI - every button maps to one IPC request to `yerdd`, so the GUI and CLI can't drift out of sync. This page is a tour of everything it can do, screen by screen; each section links to the full guide for that feature.

If you live in the terminal, the [CLI](../reference/cli/) is a first-class alternative. Not installed yet? See [Getting Started](./getting-started).

## The window at a glance

The window controls sit at the top-left of the sidebar, above the Yerd logo, the daemon's version and a status line showing whether the daemon is running. The window opens at 1312×860, or shrinks to fit inside the screen (with a small margin) when the display is smaller than that. The sidebar opens on **Overview** and lists the pages in one flat list:

- **Overview** - a live dashboard of what's running
- **Sites** · **Proxies** · **PHP** · **Node** · **Tooling** · **Services** · **Mail** · **Dumps**
- **Share** - publish a site over a public URL ([guide](./sharing))
- Pinned at the bottom: **Doctor** · **Settings**

Some rows carry a badge: Sites shows your site count, PHP and Node their available updates, Mail its unread count (click it to open the Mail viewer), Share how many sites are shared, Dumps whether interception is on or off, Doctor an amber dot when a system privilege needs elevating, and Settings a badge when a Yerd update is available.

Each page opens with a toolbar: the page title, a search box that opens the [command palette](#keyboard-shortcuts), a **Docs** link, and the page's own actions. Drag the window by the toolbar or the top of the sidebar; double-click either to zoom.

### Overview

<ThemedImage light="/images/overview-light.png" dark="/images/overview-dark.png" alt="Overview dashboard" />

The landing dashboard. Along the top, quick actions: **New site** and **Link folder** open the Sites page with that dialog ready, **Add service** does the same on the Services page, and on the right **Check for updates** and a **GitHub** link (icons only in a narrower window). The actions are disabled while the daemon is down, apart from GitHub.

Below them, a status strip shows whether Yerd is **serving**, how many `.test` sites it serves, the daemon's version, uptime, pid and running services, and health rows for the local CA, the `.test` resolver, the DNS port and the web ports (a problem row links to Doctor, or to Settings when the web or DNS ports can't be bound), with **Restart** and **Stop** buttons that ask before acting. A short setup checklist (trust the CA, install PHP, park a folder) follows until PHP is installed and a folder is parked or a site linked. If the daemon answers but its status can't be read (for example after updating only one of the app and the daemon), the strip shows the error with **Retry** and **Restart daemon**.

Next, in two columns (they stack in a narrower window), your sites card on the left and **Services** and **Contributors** on the right. The sites card has two tabs. **Pinned** lists your favourite sites, grouped under their [site groups](./sites#site-groups) with a count of how many of each group's sites are starred, and ungrouped favourites below. **Recently added** lists the last few sites you linked, created or parked, newest first; sites that were already there before you updated Yerd don't appear. Each row shows the site's domain and path (with a copy button), **Shared** and **WordPress** badges when they apply, whether it's secured, and its PHP version, plus a star to favourite or unfavourite it, a pencil that opens it on the Sites page, and an arrow that opens it in your browser. **All sites** jumps to the Sites page. The Services card lists each service with its version and port (a per-site Reverb instance also shows its site's domain) and **Start**, **Stop** and **Restart** buttons. Contributors shows the people who contribute to Yerd, by commits, and collapses with its arrow.

At the bottom, **What's new** shows a one-line summary of the version you're running. Expand it to see that release's new features and fixes on the left (long lists are trimmed, and **Full release notes** opens the rest on GitHub) and the recent releases on the right; click a release to read it.

<ThemedImage light="/images/overview-whats-new-light.png" dark="/images/overview-whats-new-dark.png" alt="The Overview scrolled down with What's new expanded: new features and fixes on the left, recent releases on the right" />

Release notes and contributors come through the daemon (and need a network connection the first time); while the daemon is down they show whatever was already loaded. When the daemon is down, a **Start Yerd** hero replaces the status strip, the sites card and Services. While the daemon is running, this page polls it every 5 seconds, so a change made from the CLI or another window shows up without a manual refresh.

<ThemedImage light="/images/overview-daemon-down-light.png" dark="/images/overview-daemon-down-dark.png" alt="The Overview while the daemon is down: the Start Yerd hero" />

<ThemedImage light="/images/overview-restart-light.png" dark="/images/overview-restart-dark.png" alt="The Restart the daemon? confirmation on the Overview" />

### Settings

<ThemedImage light="/images/settings-light.png" dark="/images/settings-dark.png" alt="Settings page" />

App- and daemon-level settings, in groups down one scrolling column. In a wide window the About information sits in a column on the right; in a narrower one it becomes a card at the top. The page stays usable when the daemon is down: the Application ports and Network groups hide until it's back. Starting, stopping and restarting the daemon live on the Overview page and in the tray menu.

Warnings appear at the top of the column: on macOS, a reminder to approve the daemon or the app under System Settings › Login Items (with an **Open Login Items** button), and, while the daemon is running, a red banner when it couldn't bind its web or DNS ports.

- **Startup.** Three switches: run the Yerd daemon at login, open the Yerd app at login, and start minimized (hidden to the tray; only available when the app opens at login). The daemon switch is disabled where no per-user service manager is available.
- **Appearance.** A System / Light / Dark **Theme** switch; a **Tray icon** selector (Automatic, Light Y, Dark Y, Full icon) for the menu bar / system tray icon; a **Title bar** selector (Automatic, macOS, Linux, Windows) that forces a window-control style regardless of host platform (the controls always sit on the left); and a **Preferred editor** selector (macOS and Linux) choosing which editor the Open-in-editor button uses - Auto-detect, any detected editor (VS Code, VSCodium, Cursor, Zed, Sublime Text, PhpStorm, Windsurf), or System default (open folder). Auto-detect picks the highest-ranked editor you have installed, preferring a dedicated PHP IDE over a general editor. **Rescan** re-runs detection if you install an editor while Yerd is open. All apply live and are remembered across launches.
- **Application ports** (while the daemon is running). Editable HTTP/HTTPS (the rootless fallback ports used when 80/443 need elevation), DNS, mail-capture, and dumps ports. Change a value and **Save & restart** asks for confirmation, validates, saves, restarts the daemon, and rechecks. HTTP/HTTPS are locked while ports are elevated, with an **Open Doctor** link to un-elevate them first.
- **Integrations and security.** **AI agents (MCP)** turns on Yerd's [MCP server](./ai-agents); while it's on, **Connect an agent** shows the registration command and config to copy. **Terminal CLI** (macOS and Linux) installs or removes `yerd` - and your installed tools (`php`, `composer`, ...) - on your shell `PATH`; on a packaged Linux install `yerd` itself is already on `PATH`, so this is mainly how Linux users get the PHP/tool shims on `PATH` too. **Symlink protection** blocks files reached through symlinks that point outside a site's folder; turning it off asks for confirmation first.
- **Network** (while the daemon is running). The outbound proxy Yerd uses for downloads, updates, Composer, npm, pnpm, Bun, ngrok, zrok, the Laravel installer and WP-CLI: detect automatically, a manual HTTP/HTTPS proxy, or none, plus a bypass list. **Test connection** checks the saved settings.
- **About.** The app, daemon, and negotiated IPC protocol versions. **Updates** shows whether you're up to date and when Yerd last checked (hover the status for the current, latest stable and latest edge versions), a Stable / Edge release-channel switch, and **Check now**; when an update is available, **Update** takes its place and downloads, verifies, and installs it, restarting the app. **Links** open yerd.app, the GitHub repository, the MIT licence, and mdbin.app (Forjed's Markdown sharing tool). **Troubleshooting**: **Logs** opens a dialog tailing the GUI's own session log (`yerd-gui.log`) alongside the daemon log, switchable between the two, with refresh and copy buttons; **Diagnostics** gathers a shareable text snapshot of app/daemon state with its own copy button - useful when reporting a problem.

<ThemedImage light="/images/settings-network-light.png" dark="/images/settings-network-dark.png" alt="Settings scrolled to Application ports, Integrations and security, and the Network proxy group" />

<ThemedImage light="/images/yerd-logs-light.png" dark="/images/yerd-logs-dark.png" alt="The Logs dialog, tailing the GUI session log" />

<ThemedImage light="/images/yerd-diagnostics-light.png" dark="/images/yerd-diagnostics-dark.png" alt="The Diagnostics dialog, a copyable JSON snapshot of app/daemon state" />

### PHP

<ThemedImage light="/images/php-light.png" dark="/images/php-dark.png" alt="The PHP page: the version list with an update banner beside PHP 8.4's inspector (settings grid, extra extensions, ini directives and Xdebug)" />

Manages your installed [PHP versions](./php-versions):

- A list of your versions, newest first, after a **Shared defaults** row: each shows a Default or Legacy chip, its patch and live FPM pool state, **Xdebug on** when its pool loads Xdebug, and an update chip or a **customised** tag. On a narrower window it becomes a table (patch, FPM, memory (RSS), Xdebug and update columns) and a version's details open in a drawer.
- **Check for updates** re-checks for patch updates; a banner under the list then offers **Update** or **Update all**. Updates are notify-only.
- **Install PHP** (or **Install another version** at the end of the list) opens one list of installable versions, supported first and legacy below, with installed ones marked. Picking a legacy version asks you to confirm you understand it's out of support. Installs download a prebuilt static build; the progress dialog can be closed and the install carries on in the background.
- A version's inspector shows its status, **Restart pool** (only when the pool is running or failed) and a `···` menu with Make default (not for legacy versions), Update (only when available) and Uninstall.
- Its **Settings** grid overrides the shared defaults for that version (empty fields inherit; changed rows get a blue dot and **Reset to shared** blanks them), plus **FPM workers** (`pm.max_children`, 1-1024, default 16, web pool only; the pool is on-demand, so a higher ceiling costs nothing while idle) and **FFI in web requests** when the build includes FFI. **Save changes**, in a bar pinned to the bottom of the inspector, restarts that version's pool. See [Per-version configuration](./php-versions#per-version-configuration) and [Pool size](./php-versions#pool-size).
- **Extra extensions** registers extra `.so` extensions for the version (loaded into both FPM and the CLI): **Add extension** chooses the file, optionally names it and toggles Zend extension. Each is load-probed before saving, and any whose file has gone missing is flagged. **Other ini directives** edits free-form directives. See [Custom extensions](./php-versions#custom-extensions).
- The **Xdebug** section turns the [bundled Xdebug](./xdebug) on or off for the version (the switch applies immediately), and picks its modes and whether it starts on every request or only on a trigger (applied with **Apply**, which leaves the switch as it is). Turning the switch on while you have unapplied mode or start edits saves those edits with it, without pressing **Apply**; turning it off keeps them staged. The first enable can take a while as Yerd downloads and checks Xdebug; the daemon gives up after 90 seconds and changes nothing. While Xdebug is on, the **Other ini directives** card notes that the Xdebug section owns `xdebug.mode` and `xdebug.start_with_request` for the web pool and that `opcache.jit` is disabled. Versions without a published build show it as not available.
- The **Shared defaults** pane edits the global ini defaults applied to every version: `memory_limit`, `upload_max_filesize`, `post_max_size`, `max_execution_time`, `max_file_uploads`, `max_input_time`, `display_errors` and `error_reporting`. Leave a field blank to use PHP's built-in default. Its **FFI in web requests** switch sets `ffi.enable` for web requests (off by default; a hint appears when libvips isn't installed), and each version can override it when its build includes FFI. Saving restarts running pools to apply; **Restart all pools** restarts every running pool.

### Node

Manages your installed [Node.js versions](./node):

<ThemedImage light="/images/node-versions-light.png" dark="/images/node-versions-dark.png" alt="The Node page: Node 24.11.1 (the default) and Node 22.21.0 with their LTS codenames and site counts, beside How a site picks its Node and Sites not on the default" />

- **Refresh** and **Install** sit in the page toolbar. The Installed versions table shows each release with chips (a star on the global default, its LTS codename, and Current or EOL once Install or Refresh has loaded the release index), how many sites use it, and any pending update. Each row's `⋯` menu offers Update, Make default, and Uninstall. Pending updates show as a count on the sidebar's Node item and as an **Update Node** item in the tray menu. Uninstall is disabled while other releases exist if the global default would then resolve to nothing (for example an exact default naming that release); its confirmation lists any site preferences it would clear.
- Beside the table (below it on narrow windows), **How a site picks its Node** shows the resolution order and **Sites not on the default** lists the sites resolving to another release, with the source (`.nvmrc`, `.node-version` or saved) and a red chip for any whose release isn't installed.
- Install opens a dialog with a dropdown of the release lines not yet installed, newest first, with LTS codenames and EOL badges (end-of-life majors only when **Show end-of-life versions** is on); it installs the chosen line's newest patch, or switch to typing an exact version. Installs stream a live log; **Hide** closes it while they keep running.
- Update installs the newest patch alongside and moves the global default to it only when the default was on an older patch of that major. Site preferences are never changed.
- A banner notes that bare `node`, `npm`, and `npx` always use the global default; run `yerd exec npm …` inside a project to use its version.

### Sites

<ThemedImage light="/images/sites-light.png" dark="/images/sites-dark.png" alt="The Sites page: sites in a Client work group, a ~/Sites folder and Linked, beside the inspector for acme-shop.test" />

The home base for [managing sites](./sites). Polls the daemon every 5 seconds while running, so sites added or changed elsewhere show up without a manual refresh.

The toolbar has **Park folder** (a native directory picker), **New group** and **New site**, which opens one two-step dialog, **Project** then **Options**, for Laravel, WordPress, Slim, CodeIgniter or CakePHP, or links an **Existing folder**. It runs one create at a time: closing the dialog or leaving the page leaves it running (it shows in the sidebar footer and a notification announces the result), and reopening **New site** shows its progress. A name an existing site already uses is refused. See [Create a new site](./sites#create-a-new-site).

<ThemedImage light="/images/create-laravel-1-light.png" dark="/images/create-laravel-1-dark.png" alt="The New site dialog's Project step with Laravel selected, the project name storefront, the ~/Sites location, PHP 8.4 and HTTPS on" />

An amber strip under the toolbar flags busy or unforwarded web ports and an untrusted local CA; its action opens the **Doctor** page, where one click runs the fix. See [HTTPS & Certificates](./https).

The list is grouped: your [groups](./sites#site-groups), then each parked folder (a folder's header can reveal or un-park it), then **Linked**. Each row shows the `name.test` domain, its path, a `+N alias` count when it answers extra domains, a lock for HTTPS, and its PHP version (amber when it isn't the default). **Filter chips** narrow the list by HTTPS, linked, framework or "not on the default PHP", and the filter field matches names, paths and **any** domain a site answers - typing `admin.` finds `codestash.test` when its only match is an added `admin.codestash.test`. With the list focused, ↑/↓ select, Return opens the site, ⌘E / Ctrl+E opens your editor, ⌘T / Ctrl+T a terminal, and ⌘C / Ctrl+C copies the URL; in the narrow table, Space or → opens the selected site's inspector. These keys ignore a selected site that a filter hides.

#### The site inspector

The selected site's details sit beside the list at the default window size, or slide in from the right as a drawer when you click a row in a [smaller window](#smaller-windows):

- **Actions.** **Open site**, a star that adds it to the Overview's favourites, the **Editor** button (macOS and Linux), **Terminal** (opens your terminal in the project directory) and **Finder** / **Files**, plus the site's URL with a copy button.
- **Environment.** PHP version; **Node** (**Default** or a release, a warning when a project file overrides the saved preference, an **Install Node X** button when the requested version isn't installed, and a tooltip reminding you to run `yerd exec node …` / `yerd exec npm …` in the project for the site's version); the **HTTPS** switch; **WordPress auto-login** with an admin picker on WordPress sites; the detected framework and how requests reach PHP; and whether the site is parked or linked.
- **Domains.** Every domain the site answers, each with **Make primary**, **Edit** and **Remove**. **Add domain** opens a dialog for new aliases, subdomains and wildcards, and **Reset to default** goes back to the apex only. See [Domains](../reference/cli/domains).
- **Response headers.** Custom [response headers](../reference/cli/headers) such as `Access-Control-Allow-Origin`, each with **Edit** and **Remove**; **Add header** opens a dialog.
- **Unpark folder…** (which stops serving every site in that parked folder) or, for a linked site, **Remove site…**, in a bar pinned to the bottom of the inspector, each with a confirmation. Your files are never deleted.

The **⋯** menu holds the rest: **Copy URL**; **Share publicly…** (a [Cloudflare Quick Tunnel](#share)); **Change web root…** (blank means auto-detect); **Move to group**, including **New group…**; **Routing rules…**, with the **Route through front controller** switch (on: everything funnels through `index.php`, as Laravel and Symfony want; off: named `.php` files execute directly) above the per-prefix [routing rules](../reference/cli/routes) that carve exceptions out of that default; **Editor**, the editor used for this site; **WP Admin** or the **Dumps viewer**; and **Unpark** or **Remove site…**.

<ThemedImage light="/images/site-actions-menu-light.png" dark="/images/site-actions-menu-dark.png" alt="The site inspector's ⋯ menu open, with the Move to group submenu showing No group, Client work and New group…" />

The **Editor** submenu defaults to the Preferred editor set in [Settings](#settings), shown as `Use default (…)`; pick a specific editor to override it for that one site, and the choice is remembered.

### Tooling

<ThemedImage light="/images/tooling-light.png" dark="/images/tooling-dark.png" alt="The Tooling page: installed Composer (update available), Laravel installer, WP-CLI, cloudflared, ngrok and zrok, with Bun (external) and pnpm available" />

Installs self-contained developer tools - Composer, Bun, pnpm, the Laravel installer, WP-CLI, cloudflared (used by [Share](#share)), and the ngrok and zrok tunnel agents - onto your PATH alongside PHP, each managed by Yerd (install / update / uninstall) so they don't collide with system installs. Node.js isn't listed here: it has its own [Node](#node) page. See [Tooling](./tooling).

### Proxies

<ThemedImage light="/images/proxies-light.png" dark="/images/proxies-dark.png" alt="Proxies page" />

Puts a `.test` address in front of a service Yerd doesn't run itself. The list has two sections, matching the two shapes of [reverse proxy](./proxies), and **All / Domains / Paths** chips to filter them:

- **Domains** - whole-domain proxies, `grafana.test` → an upstream URL. Selecting one opens its inspector: **Open** and **Copy URL**, a **Route** card with the upstream (its **Edit** button changes where the proxy points, keeping its domains and HTTPS) and an **HTTPS** switch, and a **Domains** card listing the proxy's primary, wildcard and alias domains. Its rows and **Add domain** manage the proxy's extra domains, subdomains and wildcards exactly as a site's do. A **Response headers** card adds custom [response headers](../reference/cli/headers) such as `Access-Control-Allow-Origin` to everything the proxy serves, each with **Edit** and **Remove**. **Remove proxy…**, in a bar pinned to the bottom of the inspector, asks before it removes. Proxy names may be **dotted**, so a proxy's own address can be a subdomain (`api.account.test`).
- **Paths on sites** - one path on an existing site forwarded to a service (`myapp.test/app` → Reverb), keeping everything same-origin. Its inspector shows the upstream with an **Edit** button and, pinned at the bottom, **Remove path…**. A path a service owns (such as Reverb's `/app`) shows "Managed by" instead of **Edit**, since it follows the service's port. A path's HTTPS follows its site.

**New proxy** opens one dialog with two modes. **A whole domain** takes the name (the `.test` suffix is added for you), the upstream (`http://` is assumed when you leave the scheme off) and whether to serve it over HTTPS. **A path on a site** takes the site, the path prefix (starting with `/`) and the upstream. An upstream can't be edited in place: remove the proxy and add it again.

In a narrow window the list becomes a table (forwards to, extra domains, TLS) and the inspector slides in as a drawer when you click a row. See [Reverse Proxies](./proxies).

### Services

<ThemedImage light="/images/services-light.png" dark="/images/services-dark.png" alt="Services page" />

The engines Yerd supervises - MySQL, PostgreSQL, MariaDB, Redis (Valkey), Meilisearch, and a Reverb instance per Laravel site - grouped as Databases, Cache & search and Per-site. **Add service** opens a one-step dialog (pick a tile, then the version or site, port, and Start with Yerd). Selecting a service opens its inspector: **Logs** (a live tail with Copy), Start / Restart / Stop when they apply, and a `···` menu with **Uninstall** (optionally deleting the stored data) or **Remove this instance**; a facts strip with the address and **Edit port**, the version and **Change** (or Reverb's site and **Open**; Change only moves SQL engines to newer versions, and a new PostgreSQL major starts with an empty cluster), and the **Start with Yerd** switch. SQL engines get a **Databases** card (new, filter, hide Laravel test copies, and per-row back up / restore / use in Connect / delete, confirmed in place). Every engine gets a **Connect a Laravel app** card with the `.env` block and **Copy for .env**. In a narrow window the list becomes a table and the inspector a drawer. See [Services & Databases](./services).

The **Config overrides** card lists the engine's stored [configuration overrides](./services#service-configuration-overrides) - free-form directives for its own config file, such as `max_allowed_packet` or `maxmemory-policy` - with **Add override** and a remove button per entry. Like Edit port, an override takes effect on the **next start or restart**, not immediately. The card only appears for the engines that have a config file to override; Meilisearch and Reverb are argv-driven, so they don't get it.

### Mail

<ThemedImage light="/images/mail-light.png" dark="/images/mail-dark.png" alt="The Mail page: captured mail beside an HTML order confirmation" />

The built-in SMTP **mail capture** server - point your app's mailer at `127.0.0.1` on the shown port and every outgoing email is captured for preview instead of being sent. The page is an inbox: the messages beside a reader with **Preview**, **Text** and **Headers** tabs, remote images blocked until you **Load** them (images from your `.test` sites always load), attachments and **Delete** (a table and a drawer in a narrow window). An **All apps** menu filters by sending app and the trash button clears the mailbox. The toolbar shows whether capture is running, **Setup** toggles capture, links to the port in Settings ▸ Application Ports and emits the Laravel `.env` mail keys (`MAIL_HOST`, `MAIL_PORT`, …) with editable From name/address, and the pop-out button opens the same inbox in the separate **Mails** window. Captured mail is tracked read/unread: the sidebar **Mail** item shows an unread-count pill (click it to open the Mails window), and opening a message marks it read. See [Mail Capture](./mail).

### Dumps

<ThemedImage light="/images/dumps-light.png" dark="/images/dumps-dark.png" alt="The Dumps page: captured requests beside a request's dump, query, cache, HTTP, job and log events" />

Laravel telemetry interception - `dump()`/`dd()` plus queries, jobs, views, requests, logs, cache, and outgoing HTTP - captured by a per-version PHP extension with no code changes. The page lists captured requests beside the selected request's events, with category chips, an event filter and clickable callers (a table and a drawer in a narrow window); **Keep history** keeps older requests and the trash button clears the buffer. The toolbar shows whether capture is live, **Pause / Resume** toggles it, **Setup** holds the interception switch, the captured signals, the per-version extension status and a link to the port in Settings ▸ Application Ports, and the pop-out button opens the same list and detail in the separate **Dumps** window (with pin on top). See [Laravel ▸ Dumps](./laravel-dumps).

### Share

<ThemedImage light="/images/share-light.png" dark="/images/share-dark.png" alt="The Share page with Cloudflare, ngrok and zrok provider cards, the provider filter, and acme-shop shared through both Cloudflare and ngrok" />

Publishes a local site to the public internet through a tunnel provider: Cloudflare Tunnel, ngrok or zrok. Provider cards at the top show whether each provider is set up (**Manage** opens the Providers sheet). Your shares are listed below with each one's provider icon, provider and kind, a status dot and its public address, and an **All / Cloudflare / ngrok / zrok** filter; a site can be shared through several providers at once. Selecting one shows its public URL (**Copy**, **Open**), its provider, local site and health (Connecting, Running, Reconnecting, Failed) with the last error and any preflight warnings, the site's other shares with **Also share through** for the remaining providers, and **Stop sharing** or **Remove from domain**. On a narrower window the list becomes a table and the details open in a drawer. **Share a site** picks a site and a provider (with **Set up** for one that isn't ready) and, for Cloudflare, a temporary Quick link or your own domain, or for ngrok its static domain or the URL ngrok assigns to your account; **Providers** opens the Tunnel providers sheet: the Cloudflare pane shows which `cloudflared` is in use (install or update it from [Tooling](#tooling)), connects your Cloudflare account and manages the tunnel for your domain; the ngrok pane saves an authtoken and optional static domain; the zrok pane enables this machine with a zrok account token; and **Default for new shares** picks the provider new shares use. A key button on each share sets an optional share password. See [Sharing Sites](./sharing).

### Doctor

<ThemedImage light="/images/doctor-light.png" dark="/images/doctor-dark.png" alt="Doctor page" />

Mirrors [`yerd doctor`](./diagnostics):

- **Findings.** The list groups every finding under **Problems**, **Warnings** and **Info**, each with a count. The first problem or warning is selected; the panel beside the list shows its severity, the full detail and, where there is one, a **Fix it yourself** command with a copy button. **Run safe fixes** in the toolbar applies the safe one-click repairs, and the **Re-check** button re-runs diagnostics and refreshes the permissions below. With nothing to fix, the panel shows **No problems found**; if the daemon can't run diagnostics, it shows **Health check unavailable** with a **Re-check** button.
- **System permissions.** Below the findings: Local CA trusted, `.test` resolver installed, and Privileged ports 80/443. Select one to see its state and, where it isn't granted, a **Fix** button that runs the privileged action. Once a permission *is* granted by Yerd, **Revert** undoes it, behind an in-app confirm dialog and the OS prompt. When two or more need fixing, **Fix all** on the System permissions label fixes them together (on macOS with a single password prompt for the resolver and ports). Reverting the `.test` resolver restores your previous resolver on macOS; reverting privileged ports is macOS-only (Linux `setcap` has no clean reverse, so no button is shown there).

<ThemedImage light="/images/doctor-permissions-light.png" dark="/images/doctor-permissions-dark.png" alt="Doctor with the Local CA permission selected, showing its Fix button and Fix all" />

::: info "Fix" actions never run the GUI as root
The Fix buttons run the audited `yerd elevate` helper under an OS prompt; the GUI never runs elevated. On Linux this uses `pkexec`, on macOS an `osascript … with administrator privileges` prompt. You may be asked for your password. See [Elevation & Privileges](./elevation).

On macOS, fixing Privileged ports from the app also removes any `pf` LAN redirect left by an older Yerd, without a message. If you use [LAN access](../reference/cli/lan), run `sudo yerd elevate lan` from a terminal afterwards.
:::

## Smaller windows

The pages are laid out for the default window size. Below about 1280 pixels wide, on a small laptop screen or with the window resized narrower, the list pages switch to a compact layout: the list becomes a full-width table with more columns, and the inspector that normally sits beside it becomes a drawer that slides in from the right when you click a row. Escape, the drawer's close button or a click on the backdrop closes it again. Widen the window past the breakpoint and the side-by-side layout comes back.

On the Sites page the table shows each site's **PHP**, **Node**, **Framework** and **TLS** columns, still in the same sections, and with a row selected **Space** or **→** opens its drawer:

<ThemedImage light="/images/sites-compact-light.png" dark="/images/sites-compact-dark.png" alt="The Sites page in a narrower window: filter chips above a full-width table of sites with PHP, Node, Framework and TLS columns" />

<ThemedImage light="/images/sites-compact-drawer-light.png" dark="/images/sites-compact-drawer-dark.png" alt="The same narrow Sites page with the acme-shop.test inspector pulled out as a drawer from the right over a dimmed table" />

The same applies to the other list pages: **PHP**, **Services**, **Proxies**, **Mail**, **Dumps** and **Share**.

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
| Find | `⌘F` | `Ctrl+F` | Focus the page's filter box (Sites, the Dumps window's event filter) |
| New | `⌘N` | `Ctrl+N` | Start the page's primary action (Add site, Install PHP) |
| Refresh | `⌘R` | `Ctrl+R` | Re-fetch the current page's data |
| Restart daemon | `⇧⌘R` | `Ctrl+Shift+R` | Restart `yerdd` |
| Toggle theme | `⇧⌘L` | `Ctrl+Shift+L` | Switch light / dark (applies to every window) |
| Open Mail viewer | `⇧⌘M` | `Ctrl+Shift+M` | Open the standalone Mail capture window |
| Open Dumps viewer | `⇧⌘D` | `Ctrl+Shift+D` | Open the standalone Dumps telemetry window |
| Link Site | `⇧⌘N` | `Ctrl+Shift+N` | Open the Link-site dialog on the Sites page |
| Park Folder | `⇧⌘P` | `Ctrl+Shift+P` | Open the park-folder picker on the Sites page |
| Cycle Dumps categories | `⌃⇥` / `⌃⇧⇥` | `Ctrl+Tab` / `Ctrl+Shift+Tab` | Move between category chips in the Dumps window |
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
