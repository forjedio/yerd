# PHP Versions

Yerd runs **any number of PHP versions side by side** and lets you pick which one each site uses. PHP isn't bundled, so the install stays small. The first time you ask for a version, Yerd downloads a prebuilt, statically-linked PHP build that Yerd publishes itself (signed and checksummed) and supervises one PHP-FPM pool per version behind the [reverse proxy](./sites).

## In the desktop app

The fastest way to manage PHP is the **PHP** page (under the **Environment** group) in the [desktop app](./desktop-app#php). It's a live view of every installed version and the controls to change them, with no commands to remember.

<ThemedImage light="/images/php-light.png" dark="/images/php-dark.png" alt="The PHP page in the Yerd desktop app" />

- A table of installed versions shows live FPM pool state, patch level, pool memory (RSS), and whether an update is available.
- **Install** opens a picker of installable versions (already-installed ones are hidden); progress streams live next to the Install button as the prebuilt static build downloads.
- **Refresh** re-checks for updates and **Update all** updates every version with one pending - [updates are notify-only](#updates-are-notify-only).
- Each row's `⋯` menu offers **Restart**, **Set default** (marks it with a star), **Update** (when available), and **Uninstall**; **Restart all** restarts every running pool.
- A **Default settings** card edits the [global ini defaults](#tuning-php-settings) applied to every version; leave a field blank to use PHP's built-in default, and saving restarts running pools to apply.
- A **Per-version configuration** card holds one expandable panel per installed version: the same settings form scoped to that version (empty fields inherit the defaults; see [Per-version configuration](#per-version-configuration)). Saving restarts only that version's pool.
- A **Custom extensions** card registers extra `.so` extensions per version (see [Custom extensions](#custom-extensions)); each is load-probed before it's saved, and broken registrations are flagged.

## From the command line

### Installing a version

```sh
yerd install php 8.5
```

Yerd detects your platform (`linux`/`macos`, `x86_64`/`aarch64`), fetches Yerd's signed `php.json` manifest, resolves the single published build for your platform and minor, downloads the CLI and FPM tarballs, verifies each against its published SHA-256, then atomically swaps them into place. The manifest is the source of truth for what's installable, so a new PHP patch becomes available as soon as Yerd's build pipeline publishes it.

Installs are **idempotent**: running it again replaces the directory with a fresh download of the current build. If the version isn't published for your platform, the install fails cleanly and writes nothing. The running daemon picks up a new version automatically, no restart required.

::: info A version is always a major.minor
A "PHP version" means a `major.minor` pair like `8.5`, never a full patch like `8.3.12`. Yerd installs and tracks the latest patch of the minor you ask for, and updates move you to a newer patch of that same minor. Input is `8.5` (or `php8.5`); major must be `5..=9`, minor `0..=99`.
:::

::: info Downloads are signed and hash-verified
The `php.json` manifest is signed with a dedicated minisign key whose public half is embedded in Yerd; the daemon verifies that signature before trusting the manifest, then verifies each downloaded tarball against the SHA-256 the manifest lists. Because PHP runs as you, this verification is on the install critical path, not just updates.
:::

### Bundled extensions

Yerd's builds ship the **bulk** extension set, so a real-world Laravel app has
what it needs out of the box - highlights include **`intl`** (ICU, required by
Laravel's `Number` helper), **`sodium`**, **`mysqli`**, **`gd`**, **`imagick`**,
**`redis`**, **`opcache`**, and **`swoole`**. Database access is covered by the
**`pdo_mysql`**, **`pdo_pgsql`**, and **`pdo_sqlite`** PDO drivers (so
`PDO::getAvailableDrivers()` returns all three) alongside the native `mysqli`,
`pgsql`, and `sqlite3` extensions. Coverage is provided separately by `pcov` - see
[Code Coverage](./code-coverage).

The authoritative list for any install is `php -m` (via the
[`php` shim](#the-global-default)); the full set for the current builds is below.
It rarely changes between patch updates.

<details>
<summary><b>Full bundled extension list</b> (PHP 8.5; 8.4 is identical minus <code>lexbor</code> and <code>uri</code>)</summary>

Entries marked *(core)* are part of every standard PHP build; the rest are the
extras Yerd's "bulk" build adds. Two extensions are **new in PHP 8.5** and absent
on 8.4, as noted.

| Extension | What it does |
| --- | --- |
| `apcu` | In-process shared-memory cache (APCu) for storing user data across a pool's requests. |
| `bcmath` | Arbitrary-precision decimal arithmetic for money and other exact-math needs. |
| `bz2` | bzip2 stream compression and decompression. |
| `calendar` | Conversions between calendar systems (Julian day count, Gregorian, Jewish, French). |
| `Core` | *(core)* The PHP engine itself: language constructs and built-in functions. |
| `ctype` | *(core)* Fast character-class checks such as `ctype_digit()` and `ctype_alpha()`. |
| `curl` | Network transfers via libcurl (HTTP/S, FTP, and more); the default backend for Guzzle. |
| `date` | *(core)* Date and time handling, including `DateTime` and timezone data. |
| `dba` | Key/value database abstraction over dbm-style engines (GDBM and friends). |
| `dom` | *(core)* Tree-based DOM API for reading and manipulating XML and HTML documents. |
| `event` | libevent bindings for event-driven, non-blocking I/O loops. |
| `exif` | Reads EXIF metadata (camera, orientation, GPS) embedded in image files. |
| `fileinfo` | *(core)* Detects a file's MIME type from its contents rather than its name. |
| `filter` | *(core)* Validates and sanitizes data with `filter_var()` (emails, URLs, ints, …). |
| `ftp` | Client-side FTP protocol support. |
| `gd` | Image creation and manipulation: resize, crop, draw, and convert common formats. |
| `gmp` | Arbitrary-precision integer arithmetic via GNU MP, faster than `bcmath` for big integers. |
| `hash` | *(core)* General hashing framework (`hash()`, HMAC) covering many algorithms. |
| `iconv` | *(core)* Character-set conversion between text encodings. |
| `imagick` | ImageMagick bindings for advanced image processing and a wide range of formats. |
| `imap` | Access to IMAP, POP3, and NNTP mailboxes. |
| `intl` | Unicode/ICU internationalization: number and date formatting, collation, transliteration - required by Laravel's `Number` helper. |
| `json` | *(core)* JSON encoding and decoding. |
| `lexbor` | **New in PHP 8.5.** The Lexbor HTML5 engine powering the new `\Dom\HTMLDocument` parser. |
| `libxml` | *(core)* The shared libxml2 foundation the other XML extensions build on. |
| `mbstring` | Multibyte-safe string functions for UTF-8 and other encodings. |
| `mysqli` | The improved, feature-complete MySQL/MariaDB driver. |
| `mysqlnd` | The native driver backend that `mysqli` and PDO's MySQL driver run on. |
| `openssl` | TLS, symmetric/asymmetric encryption, signatures, and X.509 certificate handling. |
| `opentelemetry` | Engine hooks that let OpenTelemetry auto-instrument code for tracing and metrics. |
| `pcntl` | Unix process control (fork, signals, `waitpid`) for CLI worker processes. |
| `pcre` | *(core)* Perl-compatible regular expressions, i.e. the `preg_*` functions. |
| `PDO` | *(core)* The database-access abstraction layer; the bundled drivers cover MySQL, PostgreSQL, and SQLite. |
| `pdo_mysql` | PDO driver for MySQL/MariaDB. |
| `pdo_pgsql` | PDO driver for PostgreSQL. |
| `pdo_sqlite` | PDO driver for SQLite. |
| `pgsql` | Native PostgreSQL client library (libpq-backed `pg_*` functions). |
| `Phar` | *(core)* PHP Archive support: bundle a whole application into one distributable file. |
| `posix` | POSIX system-call bindings (users, groups, process info) on Unix. |
| `protobuf` | Google Protocol Buffers runtime for compact, fast binary (de)serialization. |
| `random` | *(core)* The modern randomness API (`Randomizer` engines, `random_int()`). |
| `readline` | Interactive line editing and history for CLI and REPL programs. |
| `redis` | Client for the Redis / Valkey in-memory data store (phpredis). |
| `Reflection` | *(core)* Runtime introspection of classes, functions, and attributes. |
| `session` | *(core)* Server-side session state management. |
| `shmop` | Direct read/write access to shared-memory segments. |
| `SimpleXML` | *(core)* Simple object-oriented access to XML documents. |
| `soap` | SOAP client and server for XML web services. |
| `sockets` | Low-level BSD sockets API for building custom network protocols. |
| `sodium` | *(core)* Modern libsodium cryptography: authenticated encryption, signing, and hashing. |
| `SPL` | *(core)* Standard PHP Library: data-structure classes, iterators, and interfaces. |
| `sqlite3` | The self-contained, embedded SQLite database engine. |
| `standard` | *(core)* PHP's standard function library (strings, arrays, math, files, URLs, …). |
| `swoole` | Coroutine-based async runtime and high-performance server framework. |
| `sysvmsg` | System V message-queue inter-process communication. |
| `sysvsem` | System V semaphores for coordinating processes. |
| `sysvshm` | System V shared-memory inter-process communication. |
| `tokenizer` | *(core)* Tokenizes PHP source code; used by linters and static analysis tools. |
| `uri` | **New in PHP 8.5.** A built-in, spec-compliant URI parser (RFC 3986 and WHATWG). |
| `xml` | *(core)* Event-based (SAX/Expat) XML parsing. |
| `xmlreader` | *(core)* Pull-based streaming reader for large XML documents. |
| `xmlwriter` | *(core)* Streaming writer for generating XML. |
| `xsl` | XSLT 1.0 stylesheet transformations over the DOM. |
| `Zend OPcache` | Caches compiled PHP bytecode in shared memory so scripts aren't re-parsed each request (a Zend extension). |
| `zip` | Reading and writing ZIP archives. |
| `zlib` | gzip / deflate stream compression. |

</details>

Need something not in this set? Register your own with
[`yerd php ext`](#custom-extensions).

### Custom extensions

When you need an extension Yerd's builds don't ship (a PECL module like `scrypt`,
or your own compiled `.so`), register it with `yerd php ext`. Yerd loads it into
**both** the web (FPM) runtime and the CLI for that version, so `extension_loaded()`
returns `true` on a `.test` route and `php -m` lists it - the two used to diverge.

```sh
yerd php ext add 8.5 /opt/homebrew/lib/php/pecl/20250925/scrypt.so
yerd php ext list
yerd php ext remove 8.5 scrypt
```

- **Extensions are tied to a PHP version.** A native `.so` is compiled against one
  PHP *minor*, so you register it under the version it was built for (`8.5` above);
  it loads only for that version.
- **Every add is load-probed.** Before saving, Yerd actually loads the `.so` into
  that version's PHP and rejects it if it can't load - a wrong-version build, a
  missing dependency, or a Zend extension registered without `--zend` fails with a
  clear message instead of silently breaking your pools.
- **Zend extensions** (xdebug/opcache-style) use `--zend`:
  `yerd php ext add 8.5 /path/xdebug.so --zend`.
- **Naming.** The removal handle defaults to the `.so` filename (`scrypt` above);
  override it with `--name`.
- **Missing files** are handled gracefully: if a registered `.so` later disappears
  (e.g. Homebrew bumps its PECL directory on upgrade), Yerd skips it with a warning
  rather than failing to start the pool, and `yerd php ext list` marks it
  `(missing!)`.

Adding or removing an extension restarts that version's running FPM pool to apply
it. In the desktop app, the same registry lives in the **Custom extensions** card
on the **PHP** page. Registered extensions are stored per version in the config
file - see the [Configuration Reference](../reference/configuration#php).

### How versions are stored

Each install lands under the per-user data directory:

```text
{data}/php/php-8.5/bin/php          # the CLI interpreter
{data}/php/php-8.5/sbin/php-fpm     # the FastCGI process manager
{data}/php/php-8.5/.yerd-version    # the exact patch installed, e.g. "8.5.6"
{data}/bin/php                      # the default-version CLI shim
{data}/bin/php8.5                   # a per-version CLI shim
```

The dir is named for the **major.minor** (`php-8.5`); `.yerd-version` records the exact patch (`8.5.6`). Update checks read that marker to decide whether a newer patch exists. The daemon discovers installed versions by walking this directory and finding each `sbin/php-fpm` at startup.

### The global default

Yerd has one **global default** version, used for the `php` shim at `{data}/bin/php` and as the fallback for any site that hasn't pinned its own. Set it with one argument:

```sh
yerd install php 8.5
yerd use 8.5
```

A fresh config defaults to **PHP 8.3**, but you'll usually set your own right after installing.

::: tip Add the shim dir to your PATH
Put `{data}/bin` (Yerd prints the exact path) on your `PATH` so a bare `php` matches the version your sites run. The bare `php` shim resolves the current default at run time, so `yerd use` takes effect immediately with nothing to re-point.
:::

Alongside the default `php` shim, Yerd maintains a `php<version>` shim for each installed version (`php8.4`, `php8.3`, ...) so you can reach a specific version directly, plus `phpcover` / `php<version>cover` shims that run PHP with the pcov coverage driver enabled. See [Code Coverage](./code-coverage). Each shim runs the right PHP with that version's ini and any [custom extensions](#custom-extensions) you've registered.

### Per-site versions

Any site can pin its own version. Pass `yerd use` two arguments, a site name and a version:

```sh
yerd use my-app 8.3
```

Now `my-app.test` runs on 8.3 while every other site follows the global default.

| Site setting | Effective version |
|---|---|
| Pinned (`yerd use <site> 8.3`) | `8.3` |
| Not pinned | the global default |

Clearing a pin reverts the site to whatever the global default is at the time.

Check what each site resolves to with `yerd sites`, which lists every site with its kind, PHP version, HTTPS state, and document root. See [Sites](./sites) for parking and linking.

::: warning Pin a version you've installed
Pinning a site (or the default) to an uninstalled version means there's no FPM binary to start when a request arrives. Install it first (`yerd install php 8.3`), then pin. `yerd doctor` flags a pool that can't start.
:::

### Listing versions

```sh
yerd list php
```

This shows every installed version, marks the default, and flags any with a newer patch available. Update flags come from the **daemon's cache** by default, so no network call is made and the command is instant.

| Command | What you get |
|---|---|
| `yerd list php` | Installed versions, default, cached update flags (no network) |
| `yerd list php --check` | Same, but polls the distribution now to refresh update flags |
| `yerd list php --available` | Versions installable from the distribution, tagging installed ones |

`--available` takes precedence over `--check`. Add `--json` (a global flag) for machine-readable output.

### Updates are notify-only

Yerd checks for newer **patches** of the minors you have and tells you about them, but never installs on its own. The daemon periodically polls the distribution, compares each installed minor's latest patch against its `.yerd-version` marker, and on a newer patch logs:

```text
a newer PHP patch is available (run `yerd update php`)
```

It records this in the cache `yerd list php` reads. The poll is failure-tolerant: a network or platform failure is logged quietly and your cached state is left untouched.

Update on your terms:

```sh
yerd update php 8.5     # update just 8.5 to its latest patch
yerd update php         # update every installed version
```

An update is the same atomic install flow: it moves `8.5.4` → `8.5.6` and never jumps to a different minor. To move minors, run `yerd install php 8.6` and `yerd use 8.6` explicitly.

::: tip Nothing updates behind your back
Updates are strictly notify-only. The only automatic network call is the lightweight update check, which downloads nothing but a directory listing. Yerd downloads or swaps a PHP version only when you run `yerd update php`.
:::

### Tuning PHP settings

Yerd keeps a small set of **global PHP ini defaults** that are applied to *every*
installed version's FPM pool. Set and clear them with `set` / `unset`:

```sh
yerd set php memory_limit 512M
yerd set php upload_max_filesize 64M
yerd unset php memory_limit          # reset to PHP's built-in default
```

Only an allowlisted set of directives is accepted (e.g. `memory_limit`,
`max_execution_time`, `upload_max_filesize`, `post_max_size`, `display_errors`,
`error_reporting`), and the value is validated client-side before it's sent, so a
typo is a clean error rather than a broken pool. The configured values are echoed
back by `yerd list php` under a `settings:` block. See the [PHP CLI
reference](../reference/cli/php#global-php-ini-settings) for the full list and the
[Configuration Reference](../reference/configuration#php) for how they're stored
and rendered into FPM config.

### Per-version configuration

Every setting can also be pinned for a **single** installed version with the
`--php` flag - the override wins over the global default for that version only,
and applies to both its FPM pool and its CLI:

```sh
yerd set php memory_limit 1G --php 8.3   # only PHP 8.3 gets 1G
yerd unset php memory_limit --php 8.3    # 8.3 inherits the global value again
```

A per-version change restarts only that version's pool, and per-version
configuration survives uninstalling and reinstalling the version. In the
desktop app the same lives in the **Per-version configuration** card on the
PHP page: one expandable panel per installed version with the settings form
(empty fields inherit the defaults).

### Command summary

| Command | What it does |
|---|---|
| `yerd install php <version>` | Download + install the latest patch of a minor. |
| `yerd use <version>` | Set the global default version (and the `php` shim). |
| `yerd use <site> <version>` | Pin one site to a version. |
| `yerd list php [--check]` | List installed versions; `--check` refreshes update flags. |
| `yerd list php --available` | List versions installable from the distribution. |
| `yerd update php [<version>]` | Update one (or all) versions to the latest patch. |
| `yerd uninstall php <version>` | Remove a version's files (blocked if a site uses it). |
| `yerd restart php [<version>]` | Restart one (or all) running FPM pools. |
| `yerd set php <setting> <value> [--php <version>]` | Set a global PHP ini default, or a per-version override with `--php`. |
| `yerd unset php <setting> [--php <version>]` | Reset a global setting to PHP's built-in value. With `--php`, remove one version's override so the global value applies again. |
| `yerd php ext add <version> <path> [--zend] [--name <name>]` | Register a custom extension (load-probed) for a version. |
| `yerd php ext remove <version> <name>` | Remove a registered extension. |
| `yerd php ext list` | List registered custom extensions, grouped by version. |

Add `--json` to any command for machine-readable output.

## Related

- [Sites](./sites) - parking, linking, and how a request reaches an FPM pool.
- [HTTPS & Certificates](./https) - trusted HTTPS per site.
- [Diagnostics](./diagnostics) - `yerd status` and `yerd doctor` for when a pool won't start.
- [CLI Reference](../reference/cli/) - every command and flag.
- [Configuration Reference](../reference/configuration) - where the default and per-site pins live on disk.
- [yerd-php crate](../developer/crates/yerd-php) - the supervisor, version resolution, and download internals.
