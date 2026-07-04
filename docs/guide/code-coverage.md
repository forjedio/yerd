# Code Coverage

Yerd bundles [**pcov**](https://github.com/krakjoe/pcov), a fast line-coverage
driver, with every PHP version it installs - so you can run your test suite with
coverage (PHPUnit, Pest, `artisan test --coverage`) without installing or
configuring an extension yourself.

Coverage is exposed through dedicated **cover shims**: `phpcover` for your
default PHP version, and `php<version>cover` (for example `php8.4cover`) for a
specific one. They live in the same `{data}/bin` directory as the regular `php`
shim.

::: info Zero overhead by default
The plain `php` and `php<version>` shims **never** load pcov, so normal CLI
scripts and your `.test` sites run with no coverage instrumentation. pcov is
loaded only when you invoke a `…cover` shim - coverage is strictly opt-in,
per command.
:::

## Running tests with coverage

Use a cover shim anywhere you'd normally use `php`:

```sh
# Default PHP version, Pest or PHPUnit
phpcover artisan test --coverage
phpcover vendor/bin/phpunit --coverage-text

# Pin coverage to a specific PHP version
php8.4cover vendor/bin/pest --coverage
```

Each cover shim points `PHPRC` at a pcov-enabled copy of Yerd's CLI ini, then
hands off to your script. Because `PHPRC` is an environment variable rather
than a CLI flag, it's inherited by any PHP process your script spawns in
turn - which is what makes `artisan test`'s child PHPUnit/Pest/paratest run
see a working coverage driver too, not just the top-level `artisan` process.

::: tip Add the shim dir to your PATH
The cover shims sit in the same `{data}/bin` directory as `php` (Yerd prints the
exact path). Once that's on your `PATH`, `phpcover` and `php<version>cover` are
available everywhere, right next to the version shims described in
[PHP Versions](./php-versions).
:::

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

## See also

- [PHP Versions](./php-versions) - installing versions and the `php`/`php<version>` shims.
- [Laravel Dumps](./laravel-dumps) - the other extension served from `yerd-php-ext`.
