# Coverage

`yerd coverage` runs your **default** PHP version with the bundled
[pcov](../../guide/code-coverage) line-coverage driver enabled, then forwards
everything after the `coverage` subcommand verbatim to PHP. It is the discoverable
front door to the `phpcover` shim - the same coverage mechanism, but reachable
from `yerd --help` without the shim directory on your `PATH`.

Like `elevate`/`path`, it does **not** map to an IPC request: it `exec`s PHP
directly in your terminal (inheriting your stdin/stdout/stderr, arguments, and
exit code) rather than asking the daemon to do anything. (Attempting to route it
over IPC is an explicit usage error.)

```sh
yerd coverage [ARGS...]
```

| Command | Description |
| --- | --- |
| `yerd coverage [ARGS...]` | Run the default PHP version with pcov enabled, passing `ARGS` to PHP. |

```sh
yerd coverage artisan test --coverage         # Laravel / Pest / PHPUnit
yerd coverage vendor/bin/phpunit --coverage-text
yerd coverage -r 'var_dump(extension_loaded("pcov"));'   # prints bool(true)
```

The process exit code is PHP's own, so `yerd coverage` composes in CI exactly
like the interpreter it wraps.

## Pinning a version

`yerd coverage` always tracks your [global default](../../guide/php-versions#the-global-default)
version. To pin coverage to a specific version, use that version's cover shim
directly instead:

```sh
php8.4cover vendor/bin/pest --coverage
php8.5cover artisan test --coverage
```

## Passthrough behaviour

Everything after the `coverage` subcommand is handed straight to PHP, so flags
belong to your script or test runner, not to `yerd`:

- A **leading** `yerd coverage --help` (or `-h`) prints `yerd`'s own help for the
  command, because `--help` is `yerd`'s built-in flag. To forward `--help` to your
  script, put it after the script name: `yerd coverage artisan --help`.
- Every other flag - including `--version` and the global `--json` - is passed
  through to PHP. `yerd coverage` therefore produces PHP's output, never a JSON
  daemon response, and `--json` has no effect. This is the one command where the
  "`--json` on every command" note in the [overview](./) does not apply.

## Failure modes

- If the resolved default version has no published pcov build for your OS and
  architecture yet, `yerd coverage` reports that pcov isn't installed for that
  version rather than running without coverage. The background fetch is
  best-effort and never blocks a PHP install.
- **No legacy support.** pcov is not built for [legacy PHP versions](../../guide/php-versions#legacy-php-versions)
  (7.4 / 8.0 / 8.1, PHP < 8.2). `yerd coverage`, `phpcover`, and the versioned
  `php7.4cover` / `php8.0cover` / `php8.1cover` shims all error on a legacy
  version rather than run.
- **Unix only.** Coverage is available on macOS and Linux; it is not generated on
  other platforms.

## See also

- [Code Coverage guide](../../guide/code-coverage) - how pcov is bundled and how the cover shims work.
- [PHP](./php) - installing versions and setting the global default that `yerd coverage` tracks.
