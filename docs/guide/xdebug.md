# Debugging with Xdebug

Yerd ships [**Xdebug**](https://xdebug.org) 3.5 for PHP 8.2-8.5, so you can
step-debug your `.test` sites, profile slow requests, or debug a CLI command
without building or registering an extension yourself. It's a **per-version
switch**: turn it on for the PHP version you're debugging, and every other
version keeps running at full speed.

::: info Zero overhead while it's off
When a version's Xdebug is off, Yerd doesn't load it at all: that version's
PHP-FPM pool starts exactly as it would without Xdebug support. Plain `php`,
`composer` and `laravel` never load it either. Xdebug is only loaded where you
asked for it.
:::

## Turning it on

### In the desktop app

Open the **PHP** page, pick a version in the **Per-version configuration** card,
and flip the switch in its **Xdebug** section. The switch applies straight away:
the first time, Yerd downloads Xdebug for that version, checks it loads, and
restarts that version's pool with it. The installed-versions table then shows an
**Xdebug on** badge next to the version.

The same section lets you pick the [modes](#modes) and choose whether Xdebug
starts on **every request** or only **on a trigger**. Those are staged: change
them, then press **Apply**. Apply saves only what you changed and leaves the
switch as it is, so you can set up a version's modes while Xdebug is off. If
you have unapplied edits when you turn the switch **on**, they're saved with it,
no **Apply** needed. Turning it **off** leaves them staged.

The first time you turn a version on, the download and load check can take a
moment. If it hasn't finished within 90 seconds, Yerd gives up, tells you it
timed out, and changes nothing; check your connection and try again.

### From the command line

```sh
yerd xdebug on          # your default PHP version
yerd xdebug on 8.4      # a specific version
yerd xdebug status      # which versions have it on
yerd xdebug off 8.4     # back to full speed
```

Only that version's pool restarts. Sites on other versions aren't touched. See
the [`yerd xdebug` reference](../reference/cli/xdebug) for every flag.

AI agents connected through [`yerd mcp`](./ai-agents) can do the same with the
`set_xdebug` tool.

## What Yerd sets up

Out of the box, turning Xdebug on gives you:

| Setting | Value | Why |
| --- | --- | --- |
| Mode | `debug` | Step debugging. |
| Start | every request (`xdebug.start_with_request = yes`) | Your IDE gets a connection as soon as you load a page. |
| IDE address | `127.0.0.1:9003` | The Xdebug 3 default port on your own machine. |
| Logging | quiet (`xdebug.log_level = 0`) | No "Could not connect to debugging client" message on every request when your IDE isn't listening. |
| JIT | disabled (`opcache.jit = disable`) | Xdebug and the OPcache JIT can't run together. OPcache itself stays on. |

So the only thing left to do is tell your IDE to listen on port 9003 (or on
your own `xdebug.client_port`, if you [override it](#changing-the-port-and-other-settings)).

::: tip Turn it off when you're done
Even with nobody listening, Xdebug slows every request it's loaded into. Turn
it off when you've finished debugging, or switch to [trigger mode](#trigger-mode)
so it only starts when you ask.
:::

## Setting up your IDE

Your sites are served straight from your project folders, so file paths are the
same for Xdebug and your editor and **no path mappings are needed**.

### VS Code

Install the [PHP Debug](https://marketplace.visualstudio.com/items?itemName=xdebug.php-debug)
extension, then add a `.vscode/launch.json` to your project:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "name": "Listen for Xdebug",
      "type": "php",
      "request": "launch",
      "port": 9003
    }
  ]
}
```

Start **Listen for Xdebug** from the Run and Debug panel, set a breakpoint, and
load the page in your browser.

### PhpStorm

1. In **Settings → PHP → Debug**, make sure the Xdebug **Debug port** includes
   `9003` (the default is `9000,9003`).
2. Click **Start Listening for PHP Debug Connections** in the toolbar.
3. Set a breakpoint and load the page. The first time, PhpStorm asks you to
   accept the incoming connection from your site; accept it and it creates a
   server entry for that `.test` host.

::: info Opened the project through a symlink?
If you opened the project in your editor through a different path than the one
Yerd serves (for example via a symlink), Xdebug reports the real path and
breakpoints won't match. Open the real folder, or add a path mapping.
:::

## Trigger mode

Starting on every request is the easiest way in, but it means every page load
(including assets served by PHP and background `fetch` calls) tries to start a
session. **Trigger mode** only starts one when the request asks for it:

```sh
yerd xdebug on 8.4 --trigger
```

or choose the trigger option in the desktop app's Xdebug section and press
**Apply**.

The trigger is a cookie, query or POST parameter, and which one works depends
on the mode:

- **`XDEBUG_TRIGGER`** starts every mode: debugging, profiling and tracing. Add
  `?XDEBUG_TRIGGER=1` to a URL, or set it as a cookie.
- **`XDEBUG_SESSION`** only starts **step debugging**. It won't produce a
  profile or a trace.

The easiest way to set a trigger is an Xdebug helper extension for your browser
(search your browser's extension store for "Xdebug helper"): click its icon,
choose **Debug** (or **Profile** / **Trace**), and reload. Go back to
every-request mode with `yerd xdebug on 8.4 --always`.

## Modes

Xdebug does more than step debugging. Pick any combination with `--mode` (or the
checkboxes in the desktop app):

| Mode | What it does |
| --- | --- |
| `debug` | Step debugging with your IDE (the default). |
| `develop` | Development helpers: richer `var_dump()` output and stack traces on errors. |
| `coverage` | Code coverage, including branch and path coverage. |
| `profile` | Writes a cachegrind profile per request. |
| `trace` | Writes a function trace per request. |
| `gcstats` | Garbage-collection statistics. Needs the start setting on every request: Xdebug never starts GC statistics from a trigger, so `gcstats` with trigger mode is rejected. |

```sh
yerd xdebug on 8.4 --mode debug,develop
```

At least one mode is needed. To turn Xdebug off, use `yerd xdebug off` (`off` is
not a mode). Your choice is remembered, so `yerd xdebug off` followed by
`yerd xdebug on` brings back the same modes and start.

::: warning Profiling and tracing are heavy
`profile` and `trace` write a file for **every** request they run on, and slow
those requests down a lot. Pair them with `--trigger` so you only profile the
requests you choose.
:::

### Profile and trace output

When `profile` or `trace` is selected, Yerd points `xdebug.output_dir` at an
`xdebug` folder in its data directory and creates it for you:

| OS | Output folder |
| --- | --- |
| macOS | `~/Library/Application Support/io.yerd.Yerd/xdebug` |
| Linux | `~/.local/share/yerd/xdebug` (or `$XDG_DATA_HOME/yerd/xdebug`) |

Profiles are named `cachegrind.out.<pid>` and open in KCachegrind,
QCachegrind or PhpStorm's profiler viewer. The folder isn't cleaned up for you,
so delete old files when you're done. To write somewhere else, override the
directory (see the next section).

## Changing the port and other settings

The defaults suit a local IDE, but you can override them per version with
[`yerd php ini`](../reference/cli/php#custom-ini-directives). For example, to
keep an older IDE setup that listens on Xdebug 2's port:

```sh
yerd php ini set 8.4 xdebug.client_port 9000
```

Your own `xdebug.client_host`, `xdebug.client_port`, `xdebug.log_level`,
`xdebug.output_dir`, `xdebug.idekey` and other `xdebug.*` directives win over
Yerd's values. Three settings belong to the toggle and can only be changed
through it: `xdebug.mode` and `xdebug.start_with_request` (use `--mode` and
`--trigger` / `--always`), and `opcache.jit`, which stays disabled while Xdebug
is on. A directive you set with one of those names is ignored for that version's
pool until you turn Xdebug off.

## Debugging CLI commands with `phpdebug`

The web toggle only affects your sites. To debug an artisan command, a queue job
run by hand or a single test, run it through a **debug shim**:

```sh
phpdebug artisan migrate:fresh --seed
php8.4debug vendor/bin/phpunit --filter InvoiceTest
```

- **`phpdebug`** runs your [default](./php-versions#the-global-default) PHP
  version with Xdebug loaded.
- **`php<version>debug`** (for example `php8.4debug`) pins a version. There's one
  for each installed version that Xdebug is published for on your platform, so
  legacy versions and Intel Macs don't get one.

If a file Yerd didn't create already sits at `{data}/bin/phpdebug` (or at a
versioned debug alias), Yerd leaves it alone and logs a warning instead of
replacing it. Rename or remove that file and restart Yerd to get the debug shim.

They live in `{data}/bin` next to the `php` and `phpcover` shims, so once that
folder is on your `PATH` they work everywhere. They use the same defaults as the
web toggle and pick up your per-version `yerd php ini` directives, whether or not
the version's web toggle is on. Your `xdebug.*` directives override the shim's
defaults, but the OPcache JIT always stays disabled under a debug shim.

A debug shim needs that version's Xdebug download. If it isn't there yet, the
shim tells you to run `yerd xdebug on <version>` once. That also turns on
debugging for your sites; if you only want the CLI, run
`yerd xdebug off <version>` straight afterwards. The download is kept.

::: info Updates follow the web toggle
While a version's web toggle is on, Yerd checks its `xdebug.so` against the
latest release every time the daemon starts and updates it if needed. A version
you only use through `phpdebug` keeps the copy it already downloaded; turn its
toggle on once to refresh it.
:::

Only the shim's own process (and PHP processes it starts by absolute path,
such as PHPUnit's child processes) load Xdebug. A child that runs `php` from your
`PATH` gets the plain `php` shim, without Xdebug.

### Code coverage with Xdebug

Yerd also bundles pcov, which is the faster way to get line coverage: see
[Code Coverage](./code-coverage) and `phpcover`. Use Xdebug's coverage when you
need **branch or path coverage**, which pcov doesn't do. Xdebug's own
`XDEBUG_MODE` variable switches the debug shim from debugging to coverage for
one run:

```sh
XDEBUG_MODE=coverage phpdebug vendor/bin/phpunit --coverage-text
XDEBUG_MODE=coverage phpdebug vendor/bin/phpunit --path-coverage --coverage-html coverage
```

| | `phpcover` (pcov) | `XDEBUG_MODE=coverage phpdebug` |
| --- | --- | --- |
| Line coverage | yes | yes |
| Branch / path coverage | no | yes |
| Speed | fast | noticeably slower |
| Download | automatic with every PHP install | on the first `yerd xdebug on <version>` |

## Where it's available

Xdebug is built for **PHP 8.2, 8.3, 8.4 and 8.5** on:

- **macOS** on Apple Silicon
- **Linux** on x86_64 and aarch64

It isn't available for [legacy PHP](./php-versions#legacy-php-versions) (7.4,
8.0, 8.1) or on Intel Macs. Those versions show as **not available** in
`yerd xdebug status` and the desktop app, and `yerd xdebug on` explains why. The
`.so` is downloaded from the [`forjedio/yerd-php-ext`](https://github.com/forjedio/yerd-php-ext)
releases, verified by SHA-256, and stored at
`{data}/php-ext/php-<version>/xdebug.so`. Uninstalling a PHP version removes its
copy and its Xdebug settings.

## Troubleshooting

**The breakpoint is never hit.**

- Check that your IDE is actually listening on **9003**:
  `lsof -nP -iTCP:9003 -sTCP:LISTEN` should show your editor. If another program
  holds the port, stop it or [move Xdebug to another port](#changing-the-port-and-other-settings).
- Check Xdebug is on for the version that site uses: `yerd xdebug status`, and
  `yerd sites` for the site's PHP version.
- In [trigger mode](#trigger-mode), make sure the helper extension is set to
  **Debug** and reload the page.
- Xdebug connects from PHP to your IDE over `127.0.0.1`, so a firewall rarely
  gets in the way. If your IDE runs somewhere else (a VM, a container, a remote
  machine), set `xdebug.client_host` to its address with `yerd php ini set` and
  allow inbound connections on 9003 in that machine's firewall.
- To see what Xdebug is doing, point its log at a file you can write to and
  raise the log level for a moment, then read that file after loading the page:
  `yerd php ini set 8.4 xdebug.log /tmp/xdebug.log` and
  `yerd php ini set 8.4 xdebug.log_level 7`. Setting only the level writes
  nothing, because Xdebug has no log file by default. `unset` both afterwards.

**Composer or a tool drops Xdebug when run under `phpdebug`.** Composer, PHPStan,
Psalm and other tools use [`composer/xdebug-handler`](https://github.com/composer/xdebug-handler),
which restarts the process without Xdebug to keep it fast. To debug one of them,
set the tool's opt-out variable, e.g.
`COMPOSER_ALLOW_XDEBUG=1 phpdebug path/to/composer.phar install` for Composer
(other tools document their own `<TOOL>_ALLOW_XDEBUG` variable or flag). The plain
`composer` shim never loads Xdebug, so normal Composer runs aren't affected.

**"a hand-registered Xdebug … is loaded".** You registered your own `xdebug.so`
with `yerd php ext add` before Xdebug was built in. Loading two copies breaks
PHP, so Yerd refuses. Remove yours and use the bundled one:

```sh
yerd php ext list                  # find the name, e.g. xdebug
yerd php ext remove 8.4 xdebug
yerd xdebug on 8.4
```

If you had also set `xdebug.mode` with `yerd php ini`, you can remove it too
(`yerd php ini unset 8.4 xdebug.mode`): the toggle sets it now.

**"Xdebug is not built for PHP … on …".** That version or your platform isn't
[covered](#where-it-s-available). Use a supported version for the site you're
debugging.

**"couldn't download Xdebug for PHP …".** The first `on` needs to download the
extension. Check your connection, or your [outbound proxy](./corporate-networks)
if you're behind one, and try again. Nothing is saved when the download fails.

**Turning Xdebug on times out.** The download and load check have 90 seconds to
finish. When they don't, Yerd stops with an `xdebug_unavailable` error saying it
timed out, and nothing changes: Xdebug stays as it was. A slow or stalled
connection is the usual cause; try again.

**The page is slow even with no IDE open.** That's Xdebug starting a session on
every request. Turn it off, or switch to [trigger mode](#trigger-mode).

## Related

- [`yerd xdebug` reference](../reference/cli/xdebug) - every command, flag and error.
- [PHP Versions](./php-versions) - per-version settings, custom extensions and ini directives.
- [Code Coverage](./code-coverage) - pcov and the `phpcover` shims.
- [Configuration Reference](../reference/configuration#php) - how `[php.xdebug]` is stored.
