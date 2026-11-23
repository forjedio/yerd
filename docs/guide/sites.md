# Sites

A **site** is a target Yerd serves on the `.test` TLD. Each one has a name, a document root, a **served web root** (auto-detected per framework), a PHP version, and an HTTPS flag. The daemon keeps a registry and resolves every request's `Host:` header to exactly one site.

You register sites two ways:

- **Parking** points Yerd at a *parent* directory, and every child folder becomes a site (`<folder>.test`). Good for a `~/Sites` workspace you add to often.
- **Linking** registers *one* directory under a name you choose. Good for a project outside your parked workspace, or when the site name shouldn't match the folder name.

Both route identically; only the registration differs.

## In the desktop app

The **Sites** page (under **Environment** in the sidebar) is the home base for managing your sites. It lists every `.test` site in grouped sections beside an inspector for the selected site, with **Park folder**, **New group** and **New site** in the toolbar. Most day-to-day site work happens here without touching the terminal.

<ThemedImage light="/images/sites-light.png" dark="/images/sites-dark.png" alt="The Sites page in the Yerd desktop app" />

- **Park folder** opens a directory picker and parks it; **New group** creates an empty [group](#site-groups); **New site** opens the [New site](#create-a-new-site) dialog.
- An **amber strip** under the toolbar flags anything that makes sites harder to reach: web ports 80/443 busy or not forwarded (so URLs carry a port), or Yerd's local CA not trusted (browsers warn on HTTPS). Its **Run Doctor** / **Trust now** action opens Doctor (see [Diagnostics](./diagnostics)).
- **Filter chips** across the top narrow the list: **All**, **HTTPS**, **Linked**, one chip per detected framework (Laravel, WordPress, ...), and **Not on** the default PHP, each with its count. The filter field beside them (⌘F / Ctrl+F) searches names, paths and every domain a site answers on. When nothing matches, you can **Create** a site with that name or **Clear filter**.
- The list is **sectioned**: your [groups](#site-groups) first, then one section per parked folder (with its ungrouped sites), then **Linked** sites. Sections collapse, and a long section shows its first ten sites with a **+ N more** line. A folder section's header has **Reveal** and **Unpark…** (shown on hover).
- Each row shows the site's domain and path, a `+N alias` count when it answers on extra domains, a lock when it's served over HTTPS, and its PHP version (amber when it differs from the default).
- With the list focused, **↑/↓** move the selection, **Return** opens the site in your browser, **⌘E** / **Ctrl+E** opens it in your editor and **⌘T** / **Ctrl+T** in a terminal; **⌘C** / **Ctrl+C** copies its URL. In the narrow table, **Space** or **→** opens the selected site's inspector. These keys only act on a site you can see in the list.

In a wide window (the default size) the inspector sits beside the list. It keeps showing the site you picked even when a filter or a change (say, turning HTTPS off under the **HTTPS** chip) hides it from the list. In a narrower window the list becomes a table with **PHP**, **Node**, **Framework** and **TLS** columns, and clicking a row slides the inspector in from the right (Escape or the backdrop closes it). See [Smaller windows](./desktop-app#smaller-windows).

### The site inspector

- **Open site**, a **star** (adds the site to the Overview's favourites), **Editor**, **Terminal** and **Finder** (**Files** on Linux), plus a **⋯** menu.
- The site's **URL** with a copy button; a non-standard port is highlighted.
- **Environment**: the **PHP version**, the site's **Node version** (see [Node](./node#per-site-versions)), **HTTPS**, **WordPress auto-login** for WordPress sites, the detected **framework** and whether the site is **parked** or **linked**. With the keyboard, the arrow keys move between the PHP or Node versions and **Space** switches to the focused one.
- **Domains**: every domain the site answers on, tagged primary or wildcard. Each row has **Make primary** (star), **Edit** and **Remove**; **Add domain** and Edit open a small dialog, and **Reset to default** beside the heading returns the site to its apex only.
- **Response headers**: the site's custom response headers, each with **Edit** and **Remove**; **Add header** opens a dialog.
- **Unpark folder…** (a parked site) or **Remove site…** (a linked site) at the bottom. Unpark stops serving the whole parked folder, every site in it, not just this one; the confirmation says how many sites that is. Both ask for confirmation; your files are never deleted.

<ThemedImage light="/images/site-inspector-domains-light.png" dark="/images/site-inspector-domains-dark.png" alt="The site inspector scrolled to its Domains card (a primary domain, an alias, a subdomain and a wildcard) and its Response headers card" />

<ThemedImage light="/images/site-unpark-confirm-light.png" dark="/images/site-unpark-confirm-dark.png" alt="The Unpark ~/Sites? confirmation, saying 8 sites in the folder stop being served" />

The **⋯** menu has **Copy URL**, **Share publicly…**, **Change web root…** (not for WordPress), **Move to group**, **Routing rules…** (including the front-controller switch), **Editor** (a per-site editor override), **WP Admin** for WordPress sites or the **Dumps viewer** for others, and **Unpark** or **Remove site…**.

<ThemedImage light="/images/site-actions-menu-light.png" dark="/images/site-actions-menu-dark.png" alt="The site inspector's ⋯ menu open, with the Move to group submenu showing No group, Client work and New group…" />

For the full tour of the app, see [Features](./desktop-app#sites).

### Site groups

Sites can be organised into named groups - a GUI-only, cosmetic layer for scanning a large site list (client work, personal projects, whatever grouping makes sense to you). Groups don't affect routing, PHP, or HTTPS; the CLI has no concept of them.

- **Create a group and move a site into it** from the site's **⋯** menu → **Move to group** → **New group…**, then **Create & move**. **New group** in the toolbar creates an empty one.
- **Move a site** between groups, or back out with **No group**, from the same **Move to group** menu.
- **Reorder groups** with the up/down arrows next to a group's name (shown on hover).
- **Rename or delete a group** from the pencil icon next to it, which opens one **Edit group** dialog: change the name and **Save**, or click **Delete group** for a second confirmation step naming the group. Deleting a group doesn't remove its sites - they go back to their folder's section (or **Linked**).
- Groups are listed first and collapse like every section; collapsed state is remembered.
- A site with no group, or whose group was deleted, sits in its parked folder's section, or in **Linked**.

Group names are unique case-insensitively (like site names) and can't be `Unallocated`, which stays reserved.

<ThemedImage light="/images/site-new-group-light.png" dark="/images/site-new-group-dark.png" alt="The New group popover from Move to group, naming a new group Side projects with a Create & move button" />

### Link an existing folder

**New site** → **Existing folder** (or the command palette's **Link site**) opens **Link an existing folder**: choose the project folder, name the site (prefilled from the folder), and pick its PHP version and HTTPS. **Link site** registers it as a [linked](#linking-a-directory) site.

If the folder has a committed [`.yerd.toml`](./project-file), the dialog offers to apply it instead: the site name, PHP, HTTPS and the rest come from the file.

<ThemedImage light="/images/site-link-folder-light.png" dark="/images/site-link-folder-dark.png" alt="The Link an existing folder dialog with ~/Code/storefront chosen, the site name storefront, PHP 8.4 and the HTTPS switch" />

## Create a new site

Beyond registering folders you already have, the app can **scaffold a brand-new project** for you. **New site** opens one dialog in two steps, **Project** then **Options**, that runs the framework's installer under the PHP version you pick and registers the result as a `.test` site automatically.

**Project** asks for:

- **Start from** - **Laravel**, **WordPress** or **Slim**, or **Another starter** (CodeIgniter, CakePHP) from a searchable list. **Existing folder** links a folder you already have instead. Switching starters keeps everything else you've typed.
- **Project name** - the site is served at `<name>.test`; the line under it shows the URL and the folder that will be created. A name an existing site already uses (linked or parked, in any letter case) is refused.
- **Location** - pick the parent folder. If it's a [parked](#parking-a-directory) root the new site is served automatically; **Choose another folder…** picks any other folder, and the site is [linked](#linking-a-directory) there under the project name.
- **PHP version** - the version the site (and the installer) runs on. Only versions the starter supports are offered.
- **HTTPS** - serve it over TLS from day one (you can toggle this later too).

<ThemedImage light="/images/create-laravel-1-light.png" dark="/images/create-laravel-1-dark.png" alt="The New site dialog's Project step with Laravel selected, the project name storefront, the ~/Sites location, PHP 8.4 and HTTPS on" />

**Next** checks the starter's toolchain. If something is missing, a **Missing tools** step lists each tool and **Install missing tools** installs them (once, without touching your shell profile), then continues to **Options**. Tools you already have [installed externally](./tooling#external-tools), such as a global Composer, count.

**Create site** switches the dialog to a progress view: each phase ticks off with the installer's latest output under the running one, and **Hide log** / **Show log** toggles the full log. You can close the dialog, or leave the Sites page, while it runs - creation keeps going in the background, shows in the sidebar's activity indicator, and a notification announces when it finishes or fails - and reopening **New site** shows its progress. Only one site is created at a time. Likewise, closing the dialog while **Install missing tools** runs leaves the install going, and reopening shows it. When it's done, the dialog shows the URL and a summary, with **Open in editor**, **Finder** and **Open site**. If it fails, **Back** returns to Options with your choices kept.

<ThemedImage light="/images/create-laravel-3-light.png" dark="/images/create-laravel-3-dark.png" alt="The New site dialog creating storefront.test: Preflight done, Scaffolding running, and the installer's log below" />

### Laravel

::: tip Prerequisites
Creating a Laravel site needs a PHP version between 8.3 and 8.5, **Composer**, and the **Laravel installer**, which Yerd's own Composer builds. The frontend build pulls in Node, Bun, or pnpm when it's missing. pnpm also needs Node, so choosing it installs whichever of the two is missing (a Node already on your `PATH` counts).
:::

**Options** for Laravel:

- **Starter kit** - **None** (a plain skeleton, no auth scaffolding), the official **React**, **Vue**, or **Svelte** kits (Inertia + TypeScript), **Livewire** (Blade + PHP), or **Community kit** to scaffold from any `--using <package>`.
- **Authentication** (with a kit) - **Built-in**, **WorkOS**, or **None** (official kits only: installs the kit's blank variant with no auth scaffolding, so Team support and class components are hidden), plus **Team support**, and **Standalone Livewire class components** for Livewire.
- **Testing** - **Pest** or **PHPUnit**.
- **Database** - SQLite, MySQL, MariaDB, PostgreSQL, or SQL Server.
- **Frontend dependencies** - how the frontend is installed and built, for every kit (each ships a Vite build): **npm** (the default), **Bun**, **pnpm**, or **Later** to skip it.
- **Node version** - shown when two or more Yerd [Node releases](./node) are installed and the frontend isn't skipped: **Default** or a specific release, used for the build and saved as the new site's [Node version](./node#per-site-versions).
- **Initialise git** and **Laravel Boost** (installs [Boost](https://laravel.com/docs) for AI-assisted coding).

A **What Yerd will do** strip shows the `laravel new` command your choices produce and what Yerd installs around it. The progress phases are **Preflight → Scaffolding → Registering with Yerd → Ready**.

<ThemedImage light="/images/create-laravel-2-light.png" dark="/images/create-laravel-2-dark.png" alt="The New Laravel site Options step with the Vue starter kit, built-in auth, Pest, SQLite, npm, the Node version picker and the What Yerd will do strip" />

<ThemedImage light="/images/create-laravel-4-light.png" dark="/images/create-laravel-4-dark.png" alt="The finished New Laravel site dialog: storefront.test is ready, with Open in editor, Finder and Open site" />

### WordPress

::: tip Prerequisites
Creating a WordPress site needs a PHP version and **WP-CLI**. WP-CLI must be Yerd's own build - Yerd runs it directly rather than through a `wp` on your `PATH`, so an [externally installed](./tooling#external-tools) `wp` doesn't count and your own copy is left alone. Yerd's **Composer** builds WP-CLI, so it's listed too while WP-CLI still needs installing - see [Tooling](./tooling).
:::

Pick **WordPress** under **Start from** on the Project step; only PHP versions WordPress supports are offered.

<ThemedImage light="/images/create-wordpress-1-light.png" dark="/images/create-wordpress-1-dark.png" alt="The New site dialog's Project step with WordPress selected and the project name journal" />

**Options** for WordPress:

- **Site title** - WordPress's own site name, set at install time.
- **Admin username / email / password** - the first administrator account. **Generate** fills in a random password; the daemon re-validates all three server-side regardless of what the dialog sent.
- **Core version** - a specific WordPress release compatible with the chosen PHP, or **Latest**; and the **Locale** (e.g. `en_US`, `en_GB`).
- **Database engine** - **MySQL** or **MariaDB** (the only two WordPress core itself supports), preselecting one that's already running.
- **Database name** and **table prefix** - the name is derived from the project name until you edit it.

Yerd provisions the database as part of creating the site, installing/starting the chosen engine first if it isn't already running - see [Services & Databases](./services). It then runs WP-CLI's `core download`/`config create`/`core install` and sets pretty permalinks. The phases are **Preflight → Provisioning database → Downloading WordPress → Configuring → Installing → Registering with Yerd → Ready**, and the Done screen adds **WP Admin** to sign in as the administrator you just created (see below).

<ThemedImage light="/images/create-wordpress-2-light.png" dark="/images/create-wordpress-2-dark.png" alt="The New WordPress site Options step: site title, admin username, email and password, core version, locale, and the MySQL database settings" />

<ThemedImage light="/images/create-wordpress-3-light.png" dark="/images/create-wordpress-3-dark.png" alt="The New site dialog creating journal.test, with the database provisioned, WordPress downloaded and Configuring running" />

<ThemedImage light="/images/create-wordpress-4-light.png" dark="/images/create-wordpress-4-dark.png" alt="The finished New WordPress site dialog: journal.test is ready, with Open in editor, Finder, WP Admin and Open site" />

### CodeIgniter, CakePHP and Slim

These are scaffolded from the framework's official Composer template with `composer create-project`; there are no options beyond Project.

| Framework | Template | PHP |
|---|---|---|
| CodeIgniter 4 | `codeigniter4/appstarter` | 8.1+ |
| CakePHP | `cakephp/app` | 8.1+ |
| Slim 4 | `slim/skeleton` | 7.4+ |

::: tip Prerequisites
These need a PHP version the framework supports and **Composer**. An [externally installed](./tooling#external-tools) Composer works too.
:::

The web root is detected automatically: `public/` for CodeIgniter and Slim, `webroot/` for CakePHP. A new CodeIgniter site also gets a `.env` created from its `env` template, with `CI_ENVIRONMENT = development` and `app.baseURL` set to the site's URL, so errors show while you work. Yerd keeps an active `app.baseURL` in step when you later toggle HTTPS or change the site's primary domain. A commented-out one is left alone. No database is created. Configure one in the project as usual - see [Services & Databases](./services).

## WordPress one-click admin login

A WordPress site created through **New site** has **one-click admin login** turned on by default: opening **WP Admin** signs you in as the site's administrator instead of showing WordPress's own login screen. Existing or parked WordPress sites can opt in the same way.

- A WordPress site's inspector has **WP Admin** in its **⋯** menu, which opens the site's `/wp-admin/` pre-authenticated when one-click login is on.
- Turn it on or off, and choose **who** to sign in as, from the inspector's **Environment** card: the **WordPress auto-login** switch and a **Sign in as** picker (defaults to the earliest-created administrator). If the admin list can't be read (for example, the site's database is stopped), the card says so with a **Retry** link and leaves your setting as it is.

<ThemedImage light="/images/site-wordpress-light.png" dark="/images/site-wordpress-dark.png" alt="The inspector for blog.test, a WordPress site, with the WordPress auto-login switch on and Sign in as set to Alex Morgan" />

::: info How it works
Opening **WP Admin** mints a short-lived, single-use login token and appends it to the `/wp-admin/` URL. The proxy recognises and consumes the token on the first request, signing you in before redirecting - it's never valid a second time, and it does nothing outside that one request. If the resolver is off ([Localhost Access](./localhost-access)) or minting fails for any reason, **WP Admin** falls back to WordPress's ordinary login screen instead.
:::

## From the command line

Everything the Sites page does maps to a `yerd` command. These are the same operations against the same daemon, so anything you do here shows up in the app immediately.

### Parking a directory

`yerd park <dir>` registers a directory as a **parked root**. Each immediate child directory becomes a site named after the folder:

```sh
yerd park ~/Sites
#   ~/Sites/blog      ->  http://blog.test
#   ~/Sites/shop      ->  http://shop.test
#   ~/Sites/my-app    ->  http://my-app.test
#   ~/Sites/laravel.com          ->  http://laravel.test
#   ~/Sites/example.mysite.com   ->  http://example-mysite.test
```

Add a folder and its site is live; delete it and the site disappears. The child folder is the document root.

A folder named after a domain drops its public suffix: `.com`, `.co.uk`, `.app`, `.github.io` and every other suffix on the [Public Suffix List](https://publicsuffix.org/) (including hosting suffixes such as `.us.com` or `.netlify.app`). Any dots left over, and other characters a site name can't hold, become `-`. A folder with no known suffix keeps its usual name (`foo.local` is `foo-local.test`), and so does a folder that is nothing but a suffix (`com`).

If the shorter name is already taken, by another folder, a [linked](#linking-a-directory) site, a [proxy](./proxies) or a domain you added to another site, the folder keeps its full name instead. When two or more folders would shorten to the same name, none of them gets it: with both `laravel.com` and `laravel.org` parked, they are `laravel-com.test` and `laravel-org.test`, and the short name comes back once only one is left. A folder with no suffix always keeps its own name, so adding a plain `laravel` folder next to `laravel.com` moves the latter to `laravel-com.test`.

::: info Upgrading from an earlier version
Parked sites in domain-named folders change name the first time the new version starts (`laravel-com.test` becomes `laravel.test`). Yerd moves their favourite star, group, sharing hostname and password, per-site services and domain settings to the new name, and points a WordPress site's address and a CodeIgniter `app.baseURL` at the new URL once its services are running. Update bookmarks and any other URL your app stores, such as Laravel's `APP_URL`. A [proxy](./proxies) that a folder like this used to hide (a `laravel-com` proxy next to a `laravel.com` folder) starts being served once the folder takes its shorter name.
:::

::: tip
You can park multiple roots. They all contribute children to one flat namespace of `.test` names.
:::

To stop serving a parked root, un-park it. This removes only the parked root, not any linked sites:

```sh
yerd unpark ~/Sites
```

Un-parking matches the stored path exactly. List the parked roots first if you're unsure what was registered:

```sh
yerd list parked
```

::: info
`yerd list parked` shows every parked root, including empty ones. An empty root produces no sites, so it won't appear in `yerd sites`, but it's still parked.
:::

### Linking a directory

`yerd link <name> <dir>` registers a single directory as a named site. The name becomes `<name>.test`; the directory is its document root:

```sh
yerd link my-app ~/code/my-app
#   ->  http://my-app.test
```

Name and directory are both optional shorthand for the current directory:

```sh
cd ~/code/my-app

yerd link              # links the cwd, named "my-app" after its folder
yerd link my-app       # same, with an explicit name
yerd link ../other-app # links a relative path, named "other-app" after its folder
```

A folder-derived name follows the same rule as parking, so `~/code/laravel.com` links as
`laravel`. A single positional argument is treated as a directory (and the name derived from its
folder) when it contains a path separator or is `.`/`..`; otherwise it's treated as a
bare name and the current directory is linked. Web-root detection (`public/` for
Laravel, etc. - see [Web root](#web-root-the-served-directory)) runs automatically the
first time a site is linked, so a Laravel app's `SERVED` directory is usually already
correct with no extra `yerd root` step.

To remove it, unlink by name:

```sh
yerd unlink my-app
```

For a project with a committed [`.yerd.toml`](./project-file), run
[`yerd init`](../reference/cli/init) instead: it links the folder and applies the
PHP version, HTTPS, domains and services the file declares.

### Site name rules

A site name is a single DNS label, validated and lowercased before it reaches the daemon (a bad name fails as a usage error, no connection made):

- ASCII letters, digits, and hyphen only (`[a-z0-9-]`).
- No dots; a name is one label, not a domain.
- No leading or trailing hyphen.
- 1-63 characters.
- Case-insensitive: `My-App` is stored and served as `my-app`.

Valid: `my-app`, `api2`, `wp-site`. Invalid: `my.app`, `my_app`, `-app`, `app-`.

::: warning
Names are unique. Since they're lowercased first, `Foo` and `foo` collide, so the second registration is a duplicate.
:::

### Listing your sites

`yerd sites` lists every site (parked and linked) with its kind, PHP version, secure flag, served subdirectory, and document root:

```sh
yerd sites
```

```
NAME     KIND     PHP   SECURE   SERVED   DOCROOT
blog     parked   8.5   false    public   /Users/you/Sites/blog
my-app   linked   8.3   true     /        /Users/you/code/my-app
shop     parked   8.5   false    public   /Users/you/Sites/shop
```

The `SERVED` column is the web root relative to the document root; `/` means the project root itself is served.

Sites print in name order; an empty registry prints `no sites`. Add `--json` for machine-readable output:

```sh
yerd sites --json
```

### Command reference

| Command | What it does |
|---|---|
| `yerd park <dir>` | Park a directory; each child folder is served at `<name>.test`. |
| `yerd unpark <dir>` | Un-park a directory. Linked sites are untouched. |
| `yerd link [name] [dir]` | Serve a directory as a named site; both args are optional shorthand for the current directory. |
| `yerd unlink <name>` | Remove a site by name. |
| `yerd sites` | List every site (name, kind, PHP, secure, served path, doc-root). |
| `yerd list parked` | List parked roots, including empty ones. |
| `yerd secure <name>` / `yerd unsecure <name>` | Turn HTTPS on / off for a site. |
| `yerd root <name> <path>` | Set the served directory (web root) for a site. |
| `yerd root <name> --auto` | Reset a site to automatic web-root detection. |
| `yerd domain <list\|add\|remove\|primary\|reset>` | Manage a site's domains (primary, aliases, subdomains, wildcards). See [Domains](../reference/cli/domains). |

For per-site PHP, see [PHP Versions](./php-versions). For the full command surface, see the [CLI Reference](../reference/cli/sites).

## How routing works

Yerd normalises the `Host:` header and resolves it to a site using the rules below.

### The `.test` TLD

By default Yerd serves on `.test`, a reserved TLD that's safe for local development. A host only resolves if it ends in the configured TLD:

```
blog.test        ->  site "blog"
blog.example     ->  no match (wrong TLD)
blog.notthetest  ->  no match (suffix collision doesn't count)
```

The bare TLD (`test`, or `test.`) has no site label and never resolves.

::: info
The TLD is configurable (for example `dev.local`); the default is `.test`, and `yerd status` shows the active one. See [DNS &amp; .test Domains](./dns) for how `*.test` requests reach the daemon.
:::

### Host cleanup

Matching is case-insensitive and tolerant of cosmetic bits clients send. Before matching, Yerd:

- Lowercases the host (`FOO.TEST` matches `foo`).
- Strips a port (`foo.test:8443` becomes `foo.test`; a trailing `:` is fine).
- Strips one trailing FQDN dot (`foo.test.` becomes `foo.test`).

Hosts that can't be a `.test` name never match: IPv6 literals (`[::1]`), non-ASCII (`föö.test`), an empty host, a leading dot, or a malformed port (`foo.test:abc`).

### Domains, subdomains, and wildcards

By default a site answers for **exactly one** host: its apex `<name>.test`. A site can hold more than one domain, and can answer subdomains and wildcards, but each is **explicit** - you register it with [`yerd domain`](../reference/cli/domains). After confirming the host ends in `.test`, Yerd resolves it with one exact lookup, then one single-label wildcard lookup:

```text
foo.test          ->  foo            (exact apex)
corp.test         ->  foo            (only if `corp` was added to foo)
api.foo.test      ->  no match       (404 by default; subdomains are not implicit)
api.foo.test      ->  foo            (once `*.foo.test` is added to foo)
x.api.foo.test    ->  no match       (a wildcard matches one label; needs `*.api.foo.test`)
```

**This changed in v2.** Earlier builds made every subdomain fall through to its parent site implicitly. That catch-all is gone: `api.foo.test` is a 404 until you register it. Re-enable the old behavior for a site with `yerd domain add foo '*.foo.test'`. The upside is that `foo.test` and `*.foo.test` can now be **two different sites**.

**Exact beats wildcard.** The exact lookup runs first, so a registered `api.foo.test` (exact) wins over a `*.foo.test` wildcard on another site. A wildcard is never a site's primary (canonical) domain - only exact domains can be primary.

Manage a site's domains from the CLI:

```sh
yerd domain list blog                  # show blog's domains, primary marked
yerd domain add blog corp.test         # add an alias
yerd domain add blog '*.blog.test'     # add a wildcard (quote it for the shell)
yerd domain primary blog corp.test     # make corp.test the canonical address
yerd domain remove blog blog.test      # drop a domain (a site keeps >=1 exact)
yerd domain reset blog                 # back to the default apex only
```

For a WordPress site, changing the primary domain also rewrites its `siteurl`/`home`. In the desktop app the same lives in the site inspector's **Domains** card, where **Add domain** and each row's **Edit** open this dialog:

<ThemedImage light="/images/site-domain-dialog-light.png" dark="/images/site-domain-dialog-dark.png" alt="The Add domain dialog with api.acme-shop.test typed in" />

See the [domains reference](../reference/cli/domains) for the full command surface and rules.

### Document roots and the served web root

The **document root** is the project directory a site maps to: the child folder for a parked site, or the path you passed to `yerd link`. It's shown in `yerd sites`.

The directory actually served to the browser is the document root's **web root** - which, for most modern frameworks, is a subdirectory rather than the project root itself. Yerd detects this automatically (see [Web root](#web-root-the-served-directory) below), so a Laravel app parked at `~/Sites/blog` is served from `~/Sites/blog/public` without any configuration.

### The secure (HTTPS) flag

Sites start insecure (HTTP only). Securing one serves it over HTTPS with a certificate from Yerd's local CA:

```sh
yerd secure my-app      # serve over HTTPS
yerd unsecure my-app    # back to HTTP only
```

::: tip
`yerd secure` promotes a parked site to a tracked (linked) entry so the flag has somewhere to live, then flips it. See [HTTPS &amp; Certificates](./https) for how the CA and per-site certificates work.
:::

## Web root (the served directory)

Most PHP frameworks don't serve from the project root - they put a front controller in a subdirectory and keep application code out of the document root. Yerd detects the right directory automatically and serves it, so you don't hand-configure a web server:

| Framework | Served from |
|---|---|
| Laravel, Symfony (4+), CodeIgniter 4 | `public/` |
| CakePHP | `webroot/` |
| Drupal (Composer), Yii2 | `web/` |
| Magento 2 | `pub/` |
| WordPress, plain PHP | the project root |

Detection runs in the daemon when a site is registered and whenever its project changes - it reads `composer.json`, looks for framework marker files (`artisan`, `wp-config.php`, `bin/console`, …), and probes for a front controller (`index.php`) in the conventional subdirectories. A site with nothing to detect yet (an empty folder) serves the project root for now, and Yerd watches it so that **cloning a project into a parked folder makes it serve from the right directory within a second or so - no restart, no refresh**.

The served path shows up in `yerd sites` (the `SERVED` column, `/` meaning the project root itself).

::: info Static files are served directly
A request that resolves to a real file under the served root (a stylesheet, image, `favicon.ico`, compiled JS, …) is returned straight from disk by the proxy, with a guessed `Content-Type` - it never touches PHP. A directory request (including the site root) falls back to `index.html` or `index.htm` from that directory when there's no `index.php` there, so a plain static site (no PHP at all) works with no extra configuration. Everything else is handed to the framework's front controller (`index.php`). PHP source files are never served as static bytes. A symlink is allowed to point anywhere inside the site's project directory - so Laravel's `public/storage -> ../storage/app/public` link works with no extra setup - but a symlink that escapes the project directory entirely is refused with an explicit `403 Forbidden` naming the requested path, rather than being silently handed to PHP.
:::

::: info Directory requests get a trailing-slash redirect
On a site running in **direct** mode - front controller off, where named `.php` files are executable by URL - a `GET /sub` naming a real directory answers `301` to `/sub/`, exactly as Apache's `DirectorySlash` and nginx's `try_files $uri $uri/` do. The slashed form is then resolved normally: `sub/index.php` if it has one, otherwise `sub/index.html`. This is what makes a legacy multi-directory PHP app work - without it, `/sub` fell through to the *root* `index.php`, and an app whose root script redirects into the subdirectory looped until the browser gave up.

Front-controller sites are deliberately unaffected: there, `/sub` is a framework route, not a directory. The redirect is also limited to `GET` and `HEAD`, so a `POST` is never redirected.
:::

### Overriding the served path

When detection guesses wrong, or you have an unconventional layout, set the served directory explicitly:

```sh
yerd root my-app public      # serve my-app.test from <docroot>/public
yerd root my-app web/app     # a nested directory is fine
yerd root my-app --auto      # forget the override; go back to auto-detection
```

`yerd root <site>` with no path also resets to auto-detection. The path is relative to the site's directory (an absolute path inside it works too); Yerd validates that it resolves to a directory **inside** the project and rejects anything that escapes it. A manual override always wins and is never overwritten by re-detection.

::: tip In the desktop app
The [Sites view](./desktop-app#sites) shows the served path in the site inspector, and its **⋯** menu → **Change web root…** sets it directly - leave the field blank to go back to auto-detection.
:::

<ThemedImage light="/images/site-web-root-light.png" dark="/images/site-web-root-dark.png" alt="The Change web root dialog with public as the web root" />

## Routing rules

Detection and the web root cover the common layouts, but two shapes need a rule.

A **legacy portal with a nested app** - an older PHP site that grew a Yii or CodeIgniter API in a subdirectory - has *two* front controllers, and only the root one is found automatically. A rule points the rest at the nested one:

```sh
yerd route add portal /api api/index.php
# portal.test/api/user/login  → api/index.php (POST included)
# portal.test/anything-else   → the root index.php
```

A **JavaScript SPA** needs history-API deep links to serve the app shell:

```sh
yerd route add dashboard / index.html
# dashboard.test/settings/profile → index.html
```

A rule only applies when the request matched no real file, so assets and real directories keep winning. A site whose web root holds an `index.html` and no `index.php` gets the SPA behaviour automatically, with no rule at all.

In the desktop app, the site's **⋯** menu → **Routing rules…** lists the rules, adds and removes them, and holds the **Route through front controller** switch:

<ThemedImage light="/images/site-routing-rules-light.png" dark="/images/site-routing-rules-dark.png" alt="The Routing rules dialog for portal.test, with the front-controller switch on and rules sending /api to api/index.php and /reports to reports/index.html" />

See [Routing rules](../reference/cli/routes) for the full semantics, and note this is different from a [reverse proxy path rule](./proxies), which forwards to a separate running service rather than to a file inside the site.

## Response headers

A site can send extra HTTP headers on every response. The common case is CORS between two local sites, a frontend on `app.test` calling a backend on `api.test`:

```sh
yerd header set api Access-Control-Allow-Origin https://app.test
# every api.test response now carries Access-Control-Allow-Origin: https://app.test
```

A configured header replaces any same-named header the app sends, and applies to PHP responses, static files, Yerd's own error pages and redirects alike. Set headers from the **Response headers** card in the desktop app's site inspector (each row has **Edit** and **Remove**; **Add header** opens a dialog), or see [Response headers](../reference/cli/headers) for the full semantics.

<ThemedImage light="/images/site-header-dialog-light.png" dark="/images/site-header-dialog-dark.png" alt="The Add response header dialog with the name Access-Control-Allow-Credentials and the value true" />

## Streaming responses (SSE / Livewire `wire:stream`)

Streamed responses pass straight through as PHP flushes them - server-sent events, `response()->stream()`, Livewire's `wire:stream`, and token-by-token AI output all reach the browser incrementally. There is no setting to turn on and no header to send: `X-Accel-Buffering: no` is unnecessary (Yerd consumes it rather than forwarding it), and a response that sets no `Content-Length` is sent chunked.

Each open stream occupies one PHP-FPM worker for as long as it stays open, so a page holding several event streams, or a few tabs left open on one, can reach the per-version ceiling of 16 workers and leave later requests queueing. Raise it with [`yerd php pool`](./php-versions#pool-size) if you work with streaming regularly.

If a stream still arrives in one burst, the buffering is on the application side. Check `output_buffering` and any `ob_*` handlers your framework installs, and `zlib.output_compression`, which buffers to compress:

```php
while (ob_get_level() > 0) {
    ob_end_flush();
}
flush();
```

## Related

- [Project File](./project-file) - commit a `.yerd.toml` so `yerd init` sets a project up after a clone.
- [PHP Versions](./php-versions) - set the global default and pin a site to a version.
- [Routing rules](../reference/cli/routes) - nested front controllers and SPA deep links.
- [Response headers](../reference/cli/headers) - custom headers on every response, such as CORS.
- [HTTPS &amp; Certificates](./https) - the local CA and the `secure` flag.
- [DNS &amp; .test Domains](./dns) - how `*.test` requests reach the daemon.
- [Configuration Reference](../reference/configuration) - where sites and the TLD are stored.
