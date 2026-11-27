# Project File (.yerd.toml)

A `.yerd.toml` in a project's root describes how Yerd should serve that
project: its site name, PHP version, HTTPS, an optional Node version, extra
domains, response headers, path proxy rules, the services it needs and the PHP
extensions it expects. Commit it, and a teammate gets a working site with two
commands:

```sh
git clone git@github.com:acme/shop.git && cd shop
yerd init
```

`yerd init` reads the file, links the folder, installs the PHP and Node
versions it asks for when they're missing, installs and starts its services,
and reports every item as it goes. Running it again changes nothing that already
matches.

```toml
# Yerd project file. Commit it, then teammates run `yerd init`.
# Reference: https://yerd.app/reference/project-file

name = "acme"
php = "8.4"
secure = true
node = "22"
domains = ["*.acme", "admin.acme"]
extensions = ["imagick", "redis"]

[headers]
Access-Control-Allow-Origin = "*"

[proxy_rules]
"/app" = "http://127.0.0.1:3000"

[services]
mysql = "8.4"
redis = "*"
```

Every key is optional. See the [`.yerd.toml` reference](../reference/project-file)
for each field, its validation and how it is written.

::: tip `.yerd.toml` is not `yerd.toml`
[`yerd.toml`](../reference/configuration) is Yerd's own per-user state file,
owned by the daemon. `.yerd.toml` (with the leading dot) is a small file you
commit inside a project. Yerd reads it only when you apply it; it is never
consulted while serving requests.
:::

## Creating one

Run `yerd init` in a project folder that has no `.yerd.toml`. A short wizard
asks for the site name, PHP version, HTTPS, Node version, services, extra
domains and required PHP extensions, each prefilled from the site already
serving the folder (or from the folder name and your default PHP for a new
one). It then shows the file it will write and asks before writing it:

```text
$ yerd init
Site name [shop]:
PHP version [8.4]:
Serve over HTTPS? [y/N]: y
Node version (blank = not managed, `none` clears): 22
Services (comma list of engine[@version], e.g. mysql@8.4, redis): mysql@8.4, redis
Extra domains without the TLD (comma list, e.g. admin.acme, *.acme): admin.shop
Required PHP extensions (comma list, e.g. redis, imagick): redis

# Yerd project file. Commit it, then teammates run `yerd init`.
...
Write .yerd.toml and apply it? [Y/n]:
```

The site's existing response headers and path proxy rules are carried into the
file without a question. The daemon writes the file and applies it straight
away. The wizard needs a terminal: without one (or with `--json`), `yerd init`
in a folder without a file exits with a usage error.

To write the file from a site's current settings without any questions, use
**Export to .yerd.toml** in the desktop app (see [below](#in-the-desktop-app)).

## Applying one

With a `.yerd.toml` present, `yerd init` applies it without asking anything and
prints one line per item:

```text
$ yerd init
Applying /Users/me/Code/shop/.yerd.toml
applied: linked shop
installed: PHP 8.4
applied: PHP 8.4
applied: HTTPS on
applied: domain admin.shop.test
installed: Node 22
applied: Node 22
installed: mysql 8.4.9
unchanged: redis *
unchanged: extension redis
done: 8 applied, 2 unchanged, 0 warning(s), 0 failed
```

Each line starts with what happened: `applied:`, `installed:`, `started:`,
`unchanged:`, `warning:` or `failed:`. The job runs in the daemon, so the
desktop app and the CLI apply a file identically.

What apply does, in order:

1. **Site.** A folder that isn't a site yet is linked under `name` (or the
   folder name when `name` is absent, the same rule as `yerd link`). A folder
   that is already linked keeps its name. A parked folder whose name differs
   from `name` is linked under `name`; its parked PHP version and HTTPS setting
   carry over unless the file sets them, and so does a web root you set by
   hand (otherwise the linked site auto-detects it).
2. **PHP, HTTPS, domains, headers and path rules.** Each declared value is set
   on the site. Domains get your TLD appended, so `admin.acme` becomes
   `admin.acme.test`. A path rule whose prefix the site already has with a
   different upstream is repointed.
3. **Node.** The `node` selector becomes the site's saved Node preference.
4. **Services.** Each service is installed and started as needed.
5. **Extensions.** Each extension is checked against the site's PHP version.

Apply only **adds and updates**. Nothing missing from the file is removed: a
domain, header or path rule you added locally stays.

### Installs

Apply installs what the file needs and you don't have yet: a PHP version, a
Node release matching `node`, and service engines. Each install is its own
`installed:` line, and only happens when you run `yerd init` or apply from the
app. Legacy PHP versions (before 8.2) are never installed this way: that item
fails with the `yerd install php <version> --legacy` command to run first.

### Warnings and failures

Apply never overrides something that belongs elsewhere. These are reported as
`warning:` lines and the job still succeeds:

- a domain another site already serves (`...; skipped`);
- a folder linked under a different name than `name`;
- a service installed only at a version outside the declared one (with the
  `yerd service change-version` command to switch it);
- a required extension that isn't available (with the `yerd xdebug on` or
  `yerd php ext add` command to fix it);
- a `.nvmrc` or `.node-version` that pins a different Node version.

A `failed:` line is an item that couldn't be applied, such as a download
that failed while offline or a legacy PHP version that isn't installed. The
other items still run, the site stays linked, and the job ends as failed with
"N item(s) could not be applied". `yerd init` then exits `1`. Fix the cause
and run `yerd init` again.

## Services and version prefixes

A service's value is a version **prefix**, compared part by part: `"8.4"`
matches `8.4.9` but not `8.40.1`, `"8"` matches any `8.x`, and `"*"` matches
any version.

- A matching version is installed: it is started if it isn't running.
- Nothing is installed: the newest available version matching the prefix is
  installed and started.
- Only other versions are installed, or the service is set to run one that
  doesn't match: a warning, and nothing is switched.

The engines are `mysql`, `mariadb`, `postgres`, `redis` and `meilisearch`.
Apply doesn't create databases; use [`yerd db create`](../reference/cli/db).

## Keeping the file in sync

Once a project has a `.yerd.toml`, changing the site with the usual commands
or in the desktop app writes the change back into the file:

| Change | Updates |
| --- | --- |
| `yerd use`, the PHP picker | `php` |
| `yerd secure` / `unsecure`, the HTTPS switch | `secure` |
| `yerd domain add` / `remove` / `primary` / `reset` | `domains` |
| `yerd header set` / `remove` | `[headers]` |
| `yerd proxy add` / `edit` / `remove` for a path rule | `[proxy_rules]` |
| `yerd node use <v> --site`, `yerd node unset --site` | `node` |

Only the key that changed is touched. The file is only updated when it already
exists (Yerd never creates one on its own), and it is rewritten only when its
content actually changes. Making a new domain primary adds it to `domains`
(which domain is primary is not stored in the file). Services and extensions
are not written back.

::: warning Comments are not preserved
When Yerd rewrites the file it renders it fresh: keys in a fixed order, arrays
and tables sorted, and the two-line header comment at the top. Any comments or
formatting of your own are lost on the first write-back.
:::

A `.yerd.toml` that is a symlink, larger than 64 KiB, or invalid is never
rewritten, and `yerd init` refuses to apply it.

## Node precedence

The `node` key sets the site's **saved Node preference**, the same one
`yerd node use <version> --site` sets. Node's usual order still applies:

1. the nearest `.nvmrc` or `.node-version`, looking in the current directory
   and then each parent up to the project directory (in a directory that has
   both, `.nvmrc` wins; a file nearer the current directory wins over one
   further up, whichever kind it is),
2. then the saved preference (what `node` sets),
3. then the global default.

So a `.nvmrc` naming a different version wins over `node`; apply warns about it
and the wizard mentions it before the Node question. Leave `node` out to keep
Yerd from managing Node for the project at all. See [Node.js](./node#per-site-versions).

## In the desktop app

- **Link an existing folder** looks for a `.yerd.toml` as soon as you choose a
  folder or type its path. When there is one, it shows **This folder has a
  .yerd.toml** with an **Apply it after linking** switch, on by default. The
  site name then comes from the file and the PHP and HTTPS controls are hidden;
  **Link site** applies the file, and a toast reports the result with its
  warning count. Turn the switch off to link the folder under a name you
  choose, with the default PHP version and HTTPS off; the PHP and HTTPS
  controls stay hidden because changing either later also updates the
  committed `.yerd.toml`. An invalid file is reported in the dialog and you
  can still link normally. If you click **Link site** before the lookup
  finishes and it finds a file, nothing is linked yet: the dialog shows the
  file and waits for a second click.
- A site's **···** menu has **Apply .yerd.toml**, which applies the file in the
  site's folder (useful for parked sites, which never pass through the Link
  dialog), and **Export to .yerd.toml**, which writes the site's current PHP,
  HTTPS, domains, headers, path rules and Node preference to the file. Export
  keeps the file's `name`, `[services]` and `extensions`, and a `node`
  selector that matches the saved preference (so `node = "22"` stays after a
  newer 22.x is installed). When a parked folder has also been linked under
  another name, the file belongs to the linked site: exporting the parked
  one is refused with an error naming the linked site to export instead.

## Limitations

- **Parked folders keep their parked name too.** Applying a file whose `name`
  differs from a parked folder's name links the folder under `name`, but the
  parked hostname keeps routing to it as well until you unpark the parent
  directory or rename the folder.
- **Whole-host proxies aren't supported.** `[proxy_rules]` covers path rules on
  the site, including the Reverb case; whole-host proxies are global names not
  tied to a project, so add them with [`yerd proxy add`](../reference/cli/proxies).
- **Extensions are only checked.** Xdebug, FFI and custom extensions apply to
  every site on a PHP version, and a custom extension is a path on one machine,
  so apply reports what's missing and never changes them. It recognises
  extensions compiled into PHP, registered custom extensions, Xdebug when it
  is on, and the bundled pcov, which is CLI-only (use `phpcover`).
- **Node only.** Bun is a single, global tool in Yerd with no per-site version,
  so it can't be declared.
- **No per-site services.** A site's Reverb service (`reverb:<site>`) and other
  per-site services aren't part of the file.
- **Primary domains** and suppressed default domains aren't represented.
- **Symlinks are refused.** A `.yerd.toml` that is a symlink isn't read,
  applied or rewritten.
- **Comments are lost** when Yerd writes the file back (see above).

## See also

- [`.yerd.toml` reference](../reference/project-file) - every field and rule.
- [`yerd init`](../reference/cli/init) - the command, its output and exit codes.
- [Sites](./sites) - parking, linking and domains.
- [Services & Databases](./services) - installing and running engines.
