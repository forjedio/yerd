# Exec and Which

`yerd exec` runs a CLI tool under the PHP version a **site** uses, rather than
the [global default](../../guide/php-versions#the-global-default). `yerd which`
reports which binary that would be, without running anything.

A site's version governs how it is **served**, but the bare `php` and `composer`
shims always resolve to the global default. Inside a site on 8.3 while your
default is 8.5, `php artisan` and `composer install` therefore run on a different
PHP than the site's own web requests. These two commands close that gap; the
shims themselves are unchanged.

Like `coverage`, `path`, and `elevate`, neither command maps to an IPC request:
`exec` replaces itself with PHP directly (inheriting your stdin/stdout/stderr,
arguments, and exit code), and `which` only prints a path. (Attempting to route
either over IPC is an explicit usage error.) The one daemon round-trip is the
site lookup behind the scenes.

```sh
yerd exec [--site <NAME>] <php|composer> [ARGS...]
yerd which [--json] [--site <NAME>] php
```

| Command | Description |
| --- | --- |
| `yerd exec php [ARGS...]` | Run the site's PHP CLI, passing `ARGS` to PHP. |
| `yerd exec composer [ARGS...]` | Run the bundled Composer phar under the site's PHP. |
| `yerd exec --site <NAME> …` | Use the named site's version instead of the current directory's. |
| `yerd which php` | Print the absolute path of the PHP binary `yerd exec` would use. |
| `yerd which --json php` | Print `{path, version, site, source}` for that binary. |

| Flag | Description |
| --- | --- |
| `--site <NAME>` | Resolve against this site instead of the current directory. Never falls back: an unknown name is an error. |
| `--json` | `which` only. Emit the resolved path plus its version and origin as JSON. |

```sh
cd ~/Sites/my-app                # served on 8.3
yerd exec php -v                 # PHP 8.3.x
yerd exec php artisan migrate
yerd exec composer install       # the bundled phar, under 8.3
yerd exec --site blog php -v     # blog's version, from anywhere

yerd which php                   # /…/php/php-8.3/bin/php
yerd --json which php            # {"path":"…","version":"8.3","site":"my-app","source":"site"}
```

`which` shares `exec`'s resolution path exactly, so it can never print a path
that `exec` wouldn't actually run.

## Resolution

| Where you run it | Which PHP |
| --- | --- |
| Inside a registered site | that site's stored version |
| Outside every site | the global default |
| With `--site <NAME>` | that site's stored version, from anywhere |

Nested sites resolve to the **most specific** match, and matching is on the
site's project root, not its served root - so a Laravel site served from
`public/` still resolves from the project root, where `artisan` and
`composer.json` live.

Every registered site resolves to a concrete version, so `source` is `site` even
for a site you never explicitly pinned - there is no "unpinned" state. A
**linked** site snapshots the global default at link time and does **not** move
when you change the default afterwards; a **parked** site follows the current
default unless pinned. See
[Site-aware CLI](../../guide/php-versions#site-aware-cli-yerd-exec-and-yerd-which)
for why, and how to move a site's version deliberately.

In `--json` mode, `source` is `site` when the version came from a site and
`default` otherwise, with `site` then `null`.

## Passthrough behaviour

Everything after the tool is handed straight to that tool, so no `--` separator
is needed - and yerd's own flags must come **before** it:

```sh
yerd exec --site blog php -v     # --site is yerd's
yerd exec composer show --json   # --json is Composer's
yerd exec php -r 'echo PHP_VERSION;'
```

- **`-h` / `--help` go to the tool**, so `yerd exec composer --help` prints
  Composer's help, not yerd's. Use `yerd help exec` for this command's own help.
  This differs from [`coverage`](./coverage), where a leading `--help` is yerd's.
- The global `--json` is **not** interpreted by `yerd exec` - it reaches the
  tool, since `yerd exec composer show --json` has to produce Composer's JSON.
  Along with `coverage`, this is an exception to the "`--json` on every command"
  note in the [overview](./). `yerd which --json` is unaffected: `which` runs
  nothing, so `--json` is yerd's there.
- `yerd exec` exits with the tool's own exit code, so it composes in CI exactly
  like the interpreter it wraps.

`yerd exec composer` runs the same bundled phar the `composer` shim does, just
under the site's PHP - and additionally points `PHPRC` at that version's
generated CLI ini.

## Failure modes

Both commands fail rather than quietly running the wrong PHP:

- **A stored-but-uninstalled version is an error** (exit `2`), not a silent
  fallback to the default - that silent mismatch is what these commands exist to
  prevent. Install it (`yerd install php 8.3`) and retry.
- **`--site` never falls back.** An unknown name is a usage error (exit `2`);
  a daemon that isn't running to resolve it exits `69`. Naming a site is an
  instruction, not a hint.
- **Without `--site`, an unreachable daemon just means "not inside a site"** and
  the global default is used - but with a warning on stderr, since inside a site
  that would otherwise silently resolve to the wrong version.
- **Available on macOS, Linux and Windows.** On Windows `yerd exec` runs the
  tool as a child process and returns its exit code, rather than replacing
  itself with it as it does on Unix.

See [Exit codes](./#exit-codes) for the full table.

## See also

- [PHP versions guide](../../guide/php-versions#site-aware-cli-yerd-exec-and-yerd-which) - the narrative version of this page.
- [PHP](./php) - installing versions, the global default, and pinning a site.
- [Tooling](./tooling) - the `composer` shim and the rest of the shim directory.
- [Coverage](./coverage) - the other passthrough command.
