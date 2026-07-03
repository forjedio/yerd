# Sites

A **site** is a target Yerd serves on the `.test` TLD. Each one has a name, a document root, a **served web root** (auto-detected per framework), a PHP version, and an HTTPS flag. The daemon keeps a registry and resolves every request's `Host:` header to exactly one site.

You register sites two ways:

- **Parking** points Yerd at a *parent* directory, and every child folder becomes a site (`<folder>.test`). Good for a `~/Sites` workspace you add to often.
- **Linking** registers *one* directory under a name you choose. Good for a project outside your parked workspace, or when the site name shouldn't match the folder name.

Both route identically; only the registration differs.

## In the desktop app

The **Sites** page (under **Environment** in the sidebar) is the home base for managing your sites. It lists every `.test` site in a scannable table you can act on inline, with the registration controls in the header. Most day-to-day site work happens here without touching the terminal.

<ThemedImage light="/images/sites-light.png" dark="/images/sites-dark.png" alt="The Sites page in the Yerd desktop app" />

- Each row is a `name.test` site you can click to open in your browser, with a badge marking it `parked` or `linked`.
- Change a site's PHP version from a per-site picker, and flip HTTPS on or off with a one-click toggle - no commands.
- The **Served from** column shows the auto-detected web root; click it (or use the row's `⋯` menu) to set or auto-detect it.
- **Park folder** and **Link site** in the header register new sites: Park folder opens a directory picker, Link site opens a modal to name a single directory.
- A separate **Parked folders** section lists each parked root with a count of the sites it produces, plus Reveal folder and Un-park.

For the full tour of the app, see [Desktop App](./desktop-app#sites).

## Create a new Laravel site

Beyond registering folders you already have, the app can **scaffold a brand-new Laravel project** for you. Open the **Create** menu in the Sites header and choose **New Laravel site** to launch a short, four-step wizard - **Basics → Stack → Testing → Review** - that runs `laravel new` under the PHP version you pick and registers the result as a `.test` site automatically.

::: tip Prerequisites
Creating a Laravel site needs a PHP version, **Composer**, and the **Laravel installer**. If any are missing, the wizard offers to install them first; it can also use ones you already have [installed externally](./tooling#external-tools) (e.g. a global Composer). Starter kits that need Node or Bun pull the runtime in during the build.
:::

### Basics

<ThemedImage light="/images/newlaravel1-light.png" dark="/images/newlaravel1-dark.png" alt="The Create-a-new-Laravel-site wizard, Basics step" />

- **Project name** - the site is served at `<name>.test`.
- **Location** - pick the parent folder. If it's a [parked](#parking-a-directory) root the new site is served automatically; any other folder is [linked](#linking-a-directory) under the project name.
- **PHP version** - the version the site (and the installer) runs on.
- **HTTPS** - serve it over TLS from day one (you can toggle this later too).

### Stack

<ThemedImage light="/images/newlaravel2-light.png" dark="/images/newlaravel2-dark.png" alt="The Create-a-new-Laravel-site wizard, Stack step (starter kit)" />

Choose a **starter kit**: **None** (a plain skeleton, no auth scaffolding), the official **React**, **Vue**, or **Svelte** kits (Inertia + TypeScript), **Livewire** (Blade + PHP), or **Community…** to scaffold from any `--using <package>`.

### Testing

<ThemedImage light="/images/newlaravel3-light.png" dark="/images/newlaravel3-dark.png" alt="The Create-a-new-Laravel-site wizard, Testing step" />

- **Testing framework** - **Pest** or **PHPUnit**.
- **Database** - SQLite, MySQL, MariaDB, PostgreSQL, or SQL Server.
- **Initialise git** - run `git init` in the new project.
- **Laravel Boost** - install [Boost](https://laravel.com/docs) for AI-assisted coding.

### Review

A final summary of your choices. Click **Create** and Yerd runs the installer, streaming its live output so you can watch the scaffold and dependency install happen. When it finishes, the project is on disk, registered (parked or linked), served at `<name>.test`, and ready to open in your browser - no extra steps.

## From the command line

Everything the Sites page does maps to a `yerd` command. These are the same operations against the same daemon, so anything you do here shows up in the app immediately.

### Parking a directory

`yerd park <dir>` registers a directory as a **parked root**. Each immediate child directory becomes a site named after the folder:

```sh
yerd park ~/Sites
#   ~/Sites/blog      ->  http://blog.test
#   ~/Sites/shop      ->  http://shop.test
#   ~/Sites/my-app    ->  http://my-app.test
```

Add a folder and its site is live; delete it and the site disappears. The child folder is the document root.

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

To remove it, unlink by name:

```sh
yerd unlink my-app
```

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
| `yerd link <name> <dir>` | Serve a single directory as a named site. |
| `yerd unlink <name>` | Remove a site by name. |
| `yerd sites` | List every site (name, kind, PHP, secure, served path, doc-root). |
| `yerd list parked` | List parked roots, including empty ones. |
| `yerd secure <name>` / `yerd unsecure <name>` | Turn HTTPS on / off for a site. |
| `yerd root <name> <path>` | Set the served directory (web root) for a site. |
| `yerd root <name> --auto` | Reset a site to automatic web-root detection. |

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

### Subdomains and wildcards

Every site answers for its subdomains. After confirming the host ends in `.test`, Yerd takes the remaining label and:

1. Looks for an **exact** site of that name.
2. Otherwise peels the leftmost label and tries the parent, walking rightward until it finds a site or runs out of labels.

```
foo.test          ->  foo            (exact)
api.foo.test      ->  foo            (wildcard, one level)
a.b.c.foo.test    ->  foo            (wildcard, multi level)
api.bar.test      ->  no match       (no site "bar")
```

So `api.my-app.test` and `assets.my-app.test` work without registering each subdomain; they fall through to `my-app`.

### Exact match beats wildcard

If both an exact site and a wildcard parent match, the exact site wins. With `api-foo` registered alongside `foo`:

```
api-foo.test      ->  api-foo        (exact, not foo)
```

(These are different labels: `api-foo` is one label, while `api.foo` is two and wildcards to `foo`.)

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

### Overriding the served path

When detection guesses wrong, or you have an unconventional layout, set the served directory explicitly:

```sh
yerd root my-app public      # serve my-app.test from <docroot>/public
yerd root my-app web/app     # a nested directory is fine
yerd root my-app --auto      # forget the override; go back to auto-detection
```

`yerd root <site>` with no path also resets to auto-detection. The path is relative to the site's directory (an absolute path inside it works too); Yerd validates that it resolves to a directory **inside** the project and rejects anything that escapes it. A manual override always wins and is never overwritten by re-detection.

::: tip In the desktop app
The [Sites view](./desktop-app#sites) shows the served path per site and offers **Set web root…** and **Auto-detect web root** in each site's `⋯` menu.
:::

## Related

- [PHP Versions](./php-versions) - set the global default and pin a site to a version.
- [HTTPS &amp; Certificates](./https) - the local CA and the `secure` flag.
- [DNS &amp; .test Domains](./dns) - how `*.test` requests reach the daemon.
- [Configuration Reference](../reference/configuration) - where sites and the TLD are stored.
