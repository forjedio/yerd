# Code Coverage

Yerd bundles [**pcov**](https://github.com/krakjoe/pcov), a fast line-coverage
driver, with every PHP version it installs - so you can run your test suite with
coverage (PHPUnit, Pest, `artisan test --coverage`) without installing or
configuring an extension yourself.

The friendliest way in is the **`yerd coverage`** subcommand: it runs your
**default** PHP version with pcov enabled and forwards everything after the
`coverage` subcommand straight to PHP - the same coverage mechanism as the
`phpcover` shim, but discoverable from `yerd --help` without needing the shim
directory on your `PATH`.

Under the hood, coverage is exposed through dedicated **cover shims**: `phpcover`
for your default PHP version, and `php<version>cover` (for example `php8.4cover`)
for a specific one. They live in the same `{data}/bin` directory as the regular
`php` shim. `yerd coverage` runs the same coverage mechanism as `phpcover`
(default PHP + pcov); use a `php<version>cover` shim when you need to pin coverage
to a specific version.

::: info Zero overhead unless you ask for it
The plain `php` and `php<version>` shims don't load pcov on their own, so normal
CLI scripts and your `.test` sites run with no coverage instrumentation. pcov is
loaded when you invoke a `…cover` shim, or when the environment carries
[`YERD_COVER=1`](#enabling-coverage-with-yerd-cover) - which those shims export
so nested PHP runs keep coverage, and which you can set yourself. Either way
coverage is opt-in: nothing loads pcov unless something asked for it.
:::

## Running tests with coverage

Use `yerd coverage` (or a cover shim) anywhere you'd normally use `php`:

```sh
# Default PHP version, via the subcommand - args pass straight through to PHP
yerd coverage artisan test --coverage
yerd coverage vendor/bin/phpunit --coverage-text

# The same coverage mechanism, via the shim
phpcover artisan test --coverage

# Pin coverage to a specific PHP version with a versioned shim
php8.4cover vendor/bin/pest --coverage
```

::: tip `yerd coverage` is a passthrough
Everything after the `coverage` subcommand is handed verbatim to PHP, so flags
like `--coverage` belong to your test runner, not to `yerd`. Two small edges: a
leading `yerd coverage --help` prints `yerd`'s own help for the command (put
`--help` after your script to forward it, e.g. `yerd coverage artisan --help`),
and the global `--json` flag has no effect here - it, like every other flag, is
passed to PHP rather than producing a JSON response.
:::

Each cover shim points `PHPRC` at a pcov-enabled copy of Yerd's CLI ini, then
hands off to your script. Because `PHPRC` is an environment variable rather than
a CLI flag, it is inherited by any PHP process your script spawns **by absolute
interpreter path** - the `PHP_BINARY` style used by Symfony's `Process`,
paratest, and `artisan test`'s child PHPUnit/Pest run, so those see the coverage
driver too, not just the top-level `artisan` process.

A child that resolves `php` from your `PATH` instead - a `#!/usr/bin/env php`
shebang, phpunit-watcher, a `Process` handed a bare `php` - re-enters Yerd's own
`php` shim, which sets its own `PHPRC` and would otherwise drop the coverage
driver on the way through. For those, the cover shims also export
`YERD_COVER=1`, described next.

::: tip Add the shim dir to your PATH
The cover shims sit in the same `{data}/bin` directory as `php` (Yerd prints the
exact path). Once that's on your `PATH`, `phpcover` and `php<version>cover` are
available everywhere, right next to the version shims described in
[PHP Versions](./php-versions).
:::

## Enabling coverage with `YERD_COVER`

`yerd coverage` and the `phpcover` / `php<version>cover` shims set
`YERD_COVER=1` in the environment of the PHP process they exec, and every
process that one spawns inherits it. The plain `php` and `php<version>` shims
read it: when the environment carries exactly `1`, the shim loads pcov for the
PHP version **it** resolves, rather than the clean per-version ini it would
normally use. Coverage therefore survives a `PATH` hop, and a child running on a
different PHP version gets that version's own pcov build.

You can also set it yourself, which is the answer for test runners that spawn
their own PHP processes instead of being launched under a cover shim:

```sh
# Pest / PHPUnit watch mode - the watcher spawns the php runs itself
YERD_COVER=1 vendor/bin/phpunit-watcher watch

# Or turn it on for a whole shell session
export YERD_COVER=1
```

::: warning Exactly `1`, nothing else
Only the literal value `1` enables coverage. Unset, empty, `0`, `true` or any
other value means off, so a stray `YERD_COVER` can't quietly instrument
everything you run.
:::

Two limits worth knowing:

- **It applies to the `php` and `php<version>` shims.** The `composer`, `wp` and
  `laravel` shims and `yerd exec` point `PHPRC` at the clean per-version ini
  regardless, so they do not pick up `YERD_COVER`.
- **It never fails your command.** If pcov isn't available for the version the
  shim resolves - a [legacy version](./php-versions#legacy-php-versions), or a
  build that hasn't been fetched yet - the shim prints a one-line notice on
  stderr and runs your command as normal, without coverage.

## Automatic, per version

You don't install or enable anything. Whenever you install a PHP version, Yerd
fetches the matching pcov build for it in the background and (re)creates its cover
shim. The extension is downloaded from the
[`forjedio/yerd-php-ext`](https://github.com/forjedio/yerd-php-ext) releases,
verified by SHA-256, and stored alongside your PHP installs at
`{data}/php-ext/php-<version>/pcov.so` - beside the install, so a PHP **patch**
update never deletes it.

- **`phpcover`** always tracks your [global default](./php-versions#the-global-default)
  version, resolved at run time - change the default with `yerd use` and
  `phpcover` follows.
- **`php<version>cover`** is created for each installed version and removed when
  you uninstall that version.

::: info Needs a matching released build
Like the [dumps extension](./laravel-dumps), pcov is ABI-specific: one build per
PHP minor, per OS, per architecture. If a build for your exact PHP version and
platform hasn't been published yet, the cover shim reports that pcov isn't
installed for that version rather than running without coverage. The fetch is
best-effort and never blocks a PHP install.
:::

::: warning No coverage on legacy PHP
pcov isn't built for [legacy versions](./php-versions#legacy-php-versions) (7.4 /
8.0 / 8.1, PHP < 8.2). `phpcover`, `php7.4cover` / `php8.0cover` / `php8.1cover`,
and `yerd coverage` all **error** on a legacy version rather than run.
:::

::: warning Unix only
Cover shims are created on macOS and Linux only. They are not generated on other
platforms.
:::

## How it works

The `yerd` binary is a **multi-call** binary: before it parses any CLI arguments,
it checks the name it was invoked as. The `phpcover` and `php<version>cover`
entries in `{data}/bin` are symlinks back to `yerd` itself; when `yerd` sees one
of those names, it resolves the right PHP CLI binary plus that version's
`pcov.so`, writes a copy of Yerd's CLI ini with pcov's `extension`/
`pcov.enabled` directives appended, and `exec`s PHP with `PHPRC` pointing at
that copy. Invoked under any other name it falls through to the normal CLI, so
the clean `php`/`php<version>` shims are untouched.

`yerd coverage` reaches that **same** code path from the other direction: rather
than being keyed on the invoked name, the subcommand hands its forwarded
arguments to the identical cover-shim logic for the default version. So the two
front doors, subcommand and shim, share one implementation.

## See also

- [PHP Versions](./php-versions) - installing versions and the `php`/`php<version>` shims.
- [Laravel Dumps](./laravel-dumps) - the other extension served from `yerd-php-ext`.
