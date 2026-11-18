# Xdebug

`yerd xdebug` turns the **bundled Xdebug** on and off for one installed PHP
version. Yerd downloads a prebuilt, SHA-256-verified `xdebug.so` the first time
you turn a version on and loads it into that version's FPM (web) pool only while
the toggle is on. When it's off, Xdebug isn't loaded at all. The `phpdebug` /
`php<version>debug` shims load it for a single CLI command.

For the walkthrough (IDE setup, trigger mode, profiling, troubleshooting) see the
[Debugging with Xdebug](../../guide/xdebug) guide.

## Synopsis

```sh
yerd xdebug on [VERSION] [--mode <MODE>[,<MODE>...]] [--trigger | --always]
yerd xdebug off [VERSION]
yerd xdebug status
```

| Command | Description |
| --- | --- |
| `yerd xdebug on [VERSION]` | Turn Xdebug on for a version's web pool, downloading it first if needed. |
| `yerd xdebug off [VERSION]` | Turn it off. The pool restarts without Xdebug; the download is kept. |
| `yerd xdebug status` | Show every installed version as `on`, `off` or `not available`, with its modes and start trigger. |

`VERSION` is a `major.minor` (e.g. `8.4`) and must be installed. When omitted, it
defaults to your [global default](../../guide/php-versions#the-global-default)
version.

## Flags for `on`

| Flag | Description |
| --- | --- |
| `--mode <MODES>` | Comma-separated `xdebug.mode` list from `debug`, `develop`, `coverage`, `profile`, `trace`, `gcstats`. `off` is rejected (use `yerd xdebug off`), and so is an empty list. `gcstats` can't be combined with `--trigger`, because Xdebug only collects GC statistics when every request starts a session. |
| `--trigger` | Start a session only when a trigger is present (`xdebug.start_with_request = trigger`), as a cookie, query or POST parameter. `XDEBUG_TRIGGER` starts every mode; `XDEBUG_SESSION` only starts step debugging (`debug`), not profiling or tracing. |
| `--always` | Start a session on every request (`xdebug.start_with_request = yes`). Conflicts with `--trigger`. |

A flag you leave out keeps the version's current value, so `yerd xdebug on`
after `yerd xdebug off` brings back the modes and start you chose last time. A
version that has never been configured starts at the defaults: `--mode debug`
and `--always`.

```sh
yerd xdebug on                              # default PHP, step debugging on every request
yerd xdebug on 8.4 --trigger                # only when the browser sends a trigger
yerd xdebug on 8.4 --mode debug,develop     # debugging plus Xdebug's dev helpers
yerd xdebug on 8.3 --mode profile --trigger # profile the requests you trigger
yerd xdebug off 8.4
yerd xdebug status
```

## What `on` sets

While a version is on, its FPM pool starts with `zend_extension=<xdebug.so>`
and these directives on the command line:

| Directive | Value |
| --- | --- |
| `xdebug.mode` | the selected modes, comma-joined (default `debug`) |
| `xdebug.start_with_request` | `yes` (`--always`) or `trigger` (`--trigger`) |
| `xdebug.client_host` | `127.0.0.1` |
| `xdebug.client_port` | `9003` (the default; your IDE must match any override) |
| `xdebug.log_level` | `0` (no "Could not connect to debugging client" noise when no IDE is listening) |
| `opcache.jit` | `disable` (Xdebug and the JIT can't run together) |
| `xdebug.output_dir` | `{data}/xdebug`, only when `profile` or `trace` is selected |

`client_host`, `client_port`, `log_level` and `output_dir` can be overridden per
version with [`yerd php ini set`](./php#custom-ini-directives), for example
`yerd php ini set 8.4 xdebug.client_port 9000`. `xdebug.mode`,
`xdebug.start_with_request` and `opcache.jit` belong to the toggle: a free-form
directive with one of those names isn't rendered into the pool while Xdebug is
on.

## Behaviour

- **Only that version's pool restarts.** Turning a version on or off, or changing
  its modes or start while it's on, restarts that version's running pool and no
  other. A request that changes nothing doesn't save or restart anything.
  (In the desktop app you can change the modes of a version that is off; that
  just saves them for next time. The app's **Apply** sends only the fields you
  edited and never changes whether Xdebug is on.)
- **Bounded wait.** The download and load-probe have 90 seconds. Past that the
  daemon gives up with `xdebug_unavailable` ("timed out …") and nothing is
  saved, restarted or replaced.
- **Download and load-probe on first use.** The first `on` for a version
  downloads its `xdebug.so` from the
  [`forjedio/yerd-php-ext`](https://github.com/forjedio/yerd-php-ext) release,
  verifies its SHA-256, stores it at `{data}/php-ext/php-<version>/xdebug.so`,
  and load-probes it against that version's PHP before saving anything. While a
  version is on, Yerd re-checks the file against the release at every daemon
  start. Uninstalling the PHP version deletes the file and its settings.
- **Never loaded twice.** `on` refuses while you have an Xdebug registered with
  [`yerd php ext`](./php#custom-extensions) for that version (named `xdebug`, or
  any `xdebug.so`), and `yerd php ext add` refuses an Xdebug while the managed one
  is on.
- **The plain CLI is untouched.** `php`, `php<version>`, `composer`, `laravel`,
  `wp` and `yerd exec` never load Xdebug. Use the debug shims below.

`--json` prints the daemon's `PhpVersions` reply, whose `xdebug` map holds each
configured version's `{enabled, modes, start}` and whose `xdebug_versions` list
holds the installed versions Xdebug is available for. All three subcommands
reply with it.

## Availability

Xdebug is published for **PHP 8.2, 8.3, 8.4 and 8.5** on **macOS (Apple
Silicon)** and **Linux (x86_64 and aarch64)**. It isn't available for
[legacy PHP](../../guide/php-versions#legacy-php-versions) (7.4 / 8.0 / 8.1) or
on Intel Macs. `yerd xdebug status` shows those versions as `not available`.

## Errors

| Situation | Error |
| --- | --- |
| The version isn't installed | `not_found` - install it with `yerd install php <version>` first. |
| Turning on a legacy version | `legacy_restricted` - "Xdebug is not available for legacy PHP …". Turning one *off* succeeds and changes nothing. |
| No build for this version or platform | `xdebug_unavailable` - "Xdebug is not built for PHP … on …". |
| The download failed (offline, proxy, release missing the file) | `xdebug_unavailable` - "couldn't download Xdebug for PHP …". Nothing is saved. |
| A hand-registered Xdebug is loaded for that version | `already_exists` - remove it with `yerd php ext remove <version> <name>`. |
| The download or load-probe takes longer than 90 seconds | `xdebug_unavailable`, saying it timed out. Nothing is saved. |
| The downloaded `.so` fails to load | `extension_load_failed`. Nothing is saved. |
| A setting the daemon rejects, such as an empty mode list | `invalid_setting`. Nothing is saved. |
| `--mode off`, an unknown mode, or `--trigger` with `--always` | Usage error (exit code `2`), before contacting the daemon. |

## Debug shims: `phpdebug` and `php<version>debug`

The debug shims run one CLI command with Xdebug loaded, whatever the web toggle
says. They sit in `{data}/bin` next to the `php` and `phpcover` shims:

- **`phpdebug`** tracks your global default version.
- **`php<version>debug`** (e.g. `php8.4debug`) exists for each installed
  version that Xdebug is published for on this platform (not for legacy
  versions or on Intel Macs).

Yerd only manages shims it created. If `{data}/bin/phpdebug` (or a versioned
alias) already exists and isn't a Yerd shim, Yerd leaves it in place and logs a
warning rather than overwriting it.

```sh
phpdebug artisan migrate                         # step-debug an artisan command
php8.4debug vendor/bin/phpunit --filter MyTest   # step-debug a single test
XDEBUG_MODE=coverage phpdebug vendor/bin/phpunit --coverage-text
phpdebug -m | grep -i xdebug                     # prints Xdebug
```

They use the same defaults as the toggle (`debug` mode, start on every run,
`127.0.0.1:9003`, quiet logging, JIT disabled), then apply your per-version CLI
directives on top, so `yerd php ini set` overrides apply here too. Xdebug's own
`XDEBUG_MODE` environment variable overrides the mode for one run.

A debug shim needs that version's `xdebug.so` on disk. If it hasn't been
downloaded yet, the shim tells you to run `yerd xdebug on <version>` once; you
can turn it back `off` straight afterwards if you only want CLI debugging, and
the download is kept. The shims fail rather than run without Xdebug, and refuse
when a hand-registered Xdebug is already loaded for that version. They exist on
macOS and Linux only.

See [Code Coverage](../../guide/code-coverage) for `phpcover`, the faster
pcov-based coverage shim.

## See also

- [Debugging with Xdebug](../../guide/xdebug) - IDE setup, trigger mode, profiling and troubleshooting.
- [PHP](./php) - custom extensions and per-version ini directives.
- [Configuration Reference](../configuration#php) - the `[php.xdebug]` table.
