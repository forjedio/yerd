# Project file reference (.yerd.toml)

`.yerd.toml` is the committed per-project file read by [`yerd init`](./cli/init)
and the desktop app. It lives in the project's root directory (the folder you
link, not its `public/` web root). This page lists every key and rule; the
[Project File guide](../guide/project-file) explains the workflow.

::: tip Not the same as `yerd.toml`
[`yerd.toml`](./configuration) is Yerd's own per-user state, owned by the
daemon. `.yerd.toml` is a small project file you commit. Applying it writes
into the site's state in `yerd.toml`; requests are always served from
`yerd.toml`, never from the project file.
:::

## Example

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

## Fields

Every key is optional. An empty file is valid and links the folder under its
folder name.

| Key | Type | Validation | On apply | Written back by |
| --- | --- | --- | --- | --- |
| `name` | string | A [site name](../guide/sites#site-name-rules). It is normalised the way a parked folder's name is (lowercased, other characters turned into hyphens), and a name that leaves nothing valid is rejected. | Links a new folder under this name; absent means the folder name. A parked folder with another name is linked under this one. A folder already linked under another name is left as is, with a warning. | Never changed by write-back. Export fills it only when absent. |
| `php` | string | A PHP version, `"major.minor"` (for example `"8.3"`). | Installs the version if missing (not legacy versions), then sets it on the site. Absent leaves the site's PHP alone. | `yerd use` |
| `secure` | boolean | | Turns HTTPS on or off. Absent leaves it as is. | `yerd secure` / `yerd unsecure` |
| `node` | string | A Node selector: a major (`"22"`), a minor or full version (`"22.12.0"`), `"node"` (newest), `"lts/*"` or `"lts/<codename>"`. | Installs a matching release if none is, then saves it as the site's Node preference. Absent means Yerd doesn't manage Node for the project. | `yerd node use <v> --site` sets it; `yerd node unset --site` removes it. |
| `domains` | array of strings | Domains **without the TLD**: `"admin.acme"`, `"*.acme"`, `"acme-api"`. Duplicates are dropped. One that ends in your TLD (`"admin.acme.test"`) is refused before anything is applied. | Each is added to the site with your TLD appended. One another site serves is a warning. | `yerd domain add` / `remove` / `reset`; `yerd domain primary` adds a new domain (which one is primary is not stored) |
| `extensions` | array of strings | PHP module names in lowercase letters, digits and `_`. Duplicates are dropped. | Checked only, against the site's PHP version. A missing one is a warning. | Not written back. |
| `[headers]` | table, name = value | Each must be a valid [response header](./cli/headers#validation); two names that differ only in case are rejected. | Each is set on the site, replacing a header of the same name. | `yerd header set` / `remove` |
| `[proxy_rules]` | table, path prefix = URL | A path prefix starting with `/` and an `http://` or `https://` upstream, as for [`yerd proxy add`](./cli/proxies). Two rules can't share a prefix. | Each is added; a prefix the site already has with another upstream is repointed. | `yerd proxy add` / `edit` / `remove` on a path rule |
| `[services]` | table, engine = version | Engine is one of `mysql`, `mariadb`, `postgres`, `redis`, `meilisearch`. Version is `"*"` or a dotted prefix such as `"8.4"`. | See [Service versions](#service-versions). | Not written back. |

Any other key is an error naming that key, so a typo like `phpp` fails loudly
instead of being ignored. There is no schema version key.

### Service versions

A version is matched **part by part** against the exact published labels
(`8.4.9`, `17.10`):

| Value | Matches | Doesn't match |
| --- | --- | --- |
| `"8.4"` | `8.4.0`, `8.4.9` | `8.40.1`, `8.3.2` |
| `"8"` | `8.0.2`, `8.4.9` | `9.0.0` |
| `"*"` | anything | |

On apply, for each service:

- a matching version is installed: it is started when it isn't running;
- nothing is installed: the newest available version matching the value is
  installed and started;
- only non-matching versions are installed: a warning with the
  `yerd service change-version <engine> <version>` command, and nothing changes;
- the service is set to run a version that doesn't match: the same warning, even
  when a matching version is also on disk (only the version it runs counts);
- nothing is installed and no available version matches: the item fails.

### Extensions

Module names are compared case-insensitively with spaces as `_` and a leading
`zend_` dropped, so `opcache` matches PHP's `Zend OPcache`. An extension counts
as available when it is compiled into the site's PHP build, is a
[custom extension](./cli/php) registered for that version, is `xdebug` with
Xdebug turned on for that version, or is `pcov` with Yerd's bundled pcov
present for that version. pcov is CLI-only: it loads through the `phpcover` /
`php<version>cover` commands, not in `php` or PHP-FPM, and apply notes that.
Extensions loaded any other way (an `extension=` line you added to a `php.ini`
yourself) aren't detected, because the check runs PHP without ini files.
Otherwise apply warns with the fix:
`yerd xdebug on <version>` for `xdebug`, or `yerd php ext add <version> <path-to.so>`.

## How Yerd writes the file

Yerd writes `.yerd.toml` when the `yerd init` wizard finishes, when you export a
site from the desktop app, and when a change to a site is written back. Every
write renders the whole file the same way:

- the two-line header comment;
- `name`, `php`, `secure`, `node`, `domains`, `extensions`, in that order;
- then the `[headers]`, `[proxy_rules]` and `[services]` tables;
- arrays and table keys sorted, and empty keys and tables left out.

Comments and formatting of your own are not kept. A write-back only happens
when the file already exists and its parsed content changes, and only the
changed key differs. The file is written atomically (a temporary file in the
same folder, then a rename) with mode `0644`.

## Refused files

Yerd neither reads nor writes a `.yerd.toml` that:

- is a symlink or anything other than a regular file;
- is larger than 64 KiB;
- fails to parse or validate (write-back also leaves it alone).

`yerd init` and the desktop app report such a file as invalid with the reason.

## See also

- [Project File guide](../guide/project-file) - workflow, sync and limitations.
- [`yerd init`](./cli/init) - the command.
- [Configuration reference](./configuration) - Yerd's own `yerd.toml`.
