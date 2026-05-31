# yerd (CLI)

`yerd` is the command-line client. It is deliberately thin: it parses
arguments, maps each command to exactly one [`yerd-ipc`](../crates/yerd-ipc)
`Request`, exchanges that request with the [`yerdd`](./yerdd) daemon over a
local socket, and renders the `Response` as either a human-readable block or
`--json`. Almost no domain logic lives here - the daemon owns state and all
privileged work happens in [`yerd-helper`](./yerd-helper).

The one exception is `yerd elevate` / `yerd unelevate`, which do not map to a
single IPC round-trip: they orchestrate the privileged helper locally (under
`sudo`). That path lives in `elevate.rs` and is described in detail
[below](#elevate-the-privileged-orchestrator).

::: info Crate facts
Source: [`bin/yerd`](https://github.com/forjedio/yerd/tree/main/bin/yerd) ·
binary `yerd` (`src/main.rs`), library `yerd` (`src/lib.rs`). Depends on
[`yerd-core`](../crates/yerd-core), [`yerd-ipc`](../crates/yerd-ipc) (with the
`transport` feature) and [`yerd-platform`](../crates/yerd-platform). The user-
facing command catalogue lives in the [CLI Reference](../../reference/cli/).
:::

::: tip Designed fresh, not ported
The v2 command surface was designed from scratch around the daemon/IPC model -
it is **not** a port of the Yerd v1 commands. If you are migrating, see the
[Upgrade Guide](../../guide/upgrading-from-v1).
:::

## Module map

The binary is a one-line wrapper; everything testable lives in the library so
the `tests/cli_e2e.rs` integration test can drive the same code paths.

| File | Role |
| --- | --- |
| `src/main.rs` | Parse args, build a current-thread tokio runtime, call `yerd::run`. |
| `src/lib.rs` | `run()` - the orchestration: branch `elevate`, map, exchange, render, set the exit code. |
| `src/cli.rs` | The clap-derived `Cli` / `Command` surface (no I/O). |
| `src/map.rs` | Pure `to_request` (command → `Request`) and `render` (`Response` → text + exit code). |
| `src/transport.rs` | Resolve the socket path and perform one framed request/response exchange. |
| `src/elevate.rs` | `yerd elevate` / `unelevate`: local privileged orchestration of `yerd-helper`. |
| `src/error.rs` | `ClientError` - the client's error type. |

```mermaid
flowchart LR
    args["args"] --> cli["cli::Cli (clap)"]
    cli -->|"Elevate / Unelevate"| elev["elevate::run_elevate"]
    elev --> helper["spawn yerd-helper (sudo)"]
    cli -->|"everything else"| toreq["map::to_request"]
    toreq --> exch["transport::exchange"]
    exch --> render["map::render"]
    render --> out["stdout/stderr + exit code"]
```

## The command surface (`cli.rs`)

`Cli` is the top-level clap parser. The only global flag is `--json`; every
subcommand is one variant of `Command`.

```rust
#[derive(clap::Parser, Debug)]
#[command(name = "yerd", version, about = "Yerd CLI - talks to the yerdd daemon")]
pub struct Cli {
    /// Emit machine-readable JSON instead of human-readable text.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}
```

Several commands take a *target* subcommand rather than a positional argument,
which keeps room for non-PHP components later (e.g. `install php 8.5`,
`restart daemon`). The current shape:

| Command | Args / target | Notes |
| --- | --- | --- |
| `ping` | - | Liveness check. |
| `sites` | - | List parked + linked sites. |
| `park <path>` | directory | Each child directory becomes a `.test` site. |
| `link <name> <path>` | name + directory | One named site. |
| `unlink <name>` | name | Remove a linked site. |
| `unpark <path>` | directory | Remove a parked root (see canonicalisation below). |
| `use <first> [version]` | one or two args | One arg = global default PHP; two = a site's version. |
| `set php <setting> <value>` | `SetTarget::Php` | Global PHP ini default. |
| `unset php <setting>` | `UnsetTarget::Php` | Reset to PHP's built-in. |
| `install php <version>` | `InstallTarget::Php` | Download a prebuilt static build. |
| `restart php [version]` / `restart daemon` | `RestartTarget` | Pool(s), or the daemon itself. |
| `uninstall php <version>` | `UninstallTarget::Php` | Remove files; blocked if in use. |
| `list php [--check] [--available]` / `list parked` | `ListTarget` | See flag precedence below. |
| `update php [version]` | `UpdateTarget::Php` | Upgrade to latest; omit version = all. |
| `status` | - | Live daemon/proxy/DNS/ports/CA/PHP snapshot. |
| `doctor [fix]` | optional `DoctorAction::Fix` | Diagnose; `fix` attempts safe repairs. |
| `secure <name>` / `unsecure <name>` | name | Toggle HTTPS for a site. |
| `root <name> [path] [--auto]` | name + optional path | Set/reset a site's served web root → `SetWebRoot`. |
| `elevate [target]` / `unelevate [target]` | optional `ElevateTarget` | Privileged setup - handled locally. |

The `ElevateTarget` enum enumerates the three privileges: `Trust` (system CA
store), `Resolver` (`*.<tld>` DNS routing) and `Ports` (bind 80/443). Omitting
the target means "all three".

## Pure mapping and rendering (`map.rs`)

`map.rs` is the heart of the client and is entirely I/O-free, which is what
makes it exhaustively unit-testable. It has two directions.

### `to_request` - command → `Request`

```rust
pub fn to_request(cmd: &Command) -> Result<Request, ClientError>
```

Each `Command` arm produces exactly one `yerd_ipc::Request`. Crucially, this is
also where **client-side validation** happens, so a bad site name or PHP
version is a clean usage error *before* any socket connect:

- `validate_name` constructs a throwaway `Site::linked(name, "/", …)` purely to
  run the same name rules the daemon would.
- `parse_php` parses the version through `PhpVersion::FromStr`.
- `validate_php_setting` checks the setting against
  `yerd_core::php_settings::is_supported` and validates the value with
  `validate_value`.

A handful of mappings encode real design decisions worth knowing:

- **`use`** is overloaded by arity. One argument (`yerd use 8.5`) maps to
  `Request::SetDefaultPhp`; two (`yerd use blog 8.5`) maps to `Request::SetPhp`.
- **`unset php <setting>`** maps to `Request::SetPhpSettings` with an *empty
  string* value - empty value is the wire convention for "remove / reset".
- **`root <name> [path] [--auto]`** maps to `Request::SetWebRoot { name, path }`,
  where `--auto` (or omitting the path) sends `path: None` to reset the site to
  auto-detection. The name is validated client-side like `secure`/`link`.
- **`list php`** flag precedence: `--available` wins over `--check`. With
  neither, it sends `Request::ListPhp` (cached); `--check` polls the
  distribution (`CheckPhpUpdates`); `--available` lists installable versions.
- **`unpark`** passes the path through as a string here (pure); the actual
  canonicalisation happens at the I/O boundary in `run` (see below).
- **`elevate` / `unelevate`** are deliberately *not* mapped to a single
  request. Their arms return `ClientError::Usage(...)` to keep the `match`
  total; `run` branches to `elevate::run_elevate` before ever calling
  `to_request`.

### `render` - `Response` → text + exit code

```rust
pub fn render(resp: &Response, json: bool) -> Rendered

pub struct Rendered { pub stdout: String, pub stderr: String, pub code: u8 }
```

`render` formats a `Response` into stdout/stderr and a process exit code. Two
properties are intentional and tested:

1. **The exit code is computed once, before branching on `--json`,** so the
   JSON and human paths always agree. `doctor_exit_code` returns `1` for a
   `Response::Error`, `1` for any `Severity::Fail` doctor finding (in
   `Diagnoses` or a `DoctorFix` report's `manual` list), else `0`.
2. **`Response` is `#[non_exhaustive]`.** An unknown variant from a newer daemon
   falls through to a benign `"unexpected response from daemon"` on stderr
   rather than panicking.

The formatters render tab-separated tables (`sites`), annotated version lists
(marking the default and any available updates), and a multi-line `status`
block. `status` carries small but real semantics - for example `fmt_port`
distinguishes a plain rootless fallback (`80 → 8080 (fallback)`) from a macOS
`pf` redirect that makes the privileged port reachable (`80 → 8080
(redirected)`), and empty `daemon_version` (an older daemon, `#[serde(default)]`)
renders as `unknown` rather than blank.

## Transport (`transport.rs`)

The client and daemon must agree on the socket path *without* a config
exchange. They do so by deriving it identically from
`yerd_platform::Paths::resolve()`:

```rust
#[cfg(unix)]
pub async fn exchange(req: &Request) -> Result<Response, ClientError> {
    let dirs = ActivePaths::new().resolve()?;
    exchange_at(&dirs.runtime.join("yerd.sock"), req).await
}
```

`exchange_at` is factored out so integration tests can target a tempdir socket.
It connects with `interprocess` (`GenericFilePath`), then frames one request and
reads one response using `yerd-ipc`'s `write_message` / `read_message` /
`FrameDecoder` with `DEFAULT_MAX_FRAME`. A closed connection with no reply
becomes `ClientError::DaemonUnreachable`.

::: warning Windows is not yet a client
`exchange` on non-Unix targets returns `DaemonUnreachable` immediately: the
daemon's Windows pipe name is currently PID-based and not derivable by a client.
This is tracked as a Phase-2 follow-up - treat Windows CLI support as roadmap.
:::

## Orchestration and exit codes (`lib.rs`)

`run` ties the pieces together. It does **not** auto-start the daemon - if the
socket is unreachable it reports an error (or, for `doctor`, a synthetic FAIL).
The flow:

1. Branch `Elevate` / `Unelevate` to `elevate::run_elevate` and return.
2. `map::to_request`; on `Err` print `yerd: <e>` and exit `2`.
3. `canonicalize_unpark` rewrites an `Unpark` request's path to its canonical
   form at the I/O boundary, so a relative/symlinked path the user typed matches
   the canonical string the daemon stored when the directory was parked. (The
   daemon matches `unpark` *exactly* and deliberately does not canonicalise, so
   an already-deleted directory is still removable by its stored path.)
4. `transport::exchange`, then `map::render`, then print and exit with the
   rendered code.

Two extra behaviours live here:

- After a successful global `yerd use <ver>` (human output only),
  `print_php_path_hint` prints where the managed `php` shim lives and warns if a
  different `php` shadows it earlier on `PATH`.
- For `doctor` specifically, a down daemon is itself a FAIL: `run` synthesises a
  `DiagnosisCode::DaemonDown` `Diagnoses` response and renders it through the
  normal path, so `--json` and the exit code behave like any other doctor run
  (exit `1`) instead of the generic "unreachable" code.

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success. |
| `1` | Daemon error response, or a doctor `Fail` finding. |
| `2` | Client-side usage error (bad name/version/setting - before connecting). |
| `69` | Daemon unreachable (`EX_UNAVAILABLE`). |
| `70` | Could not build the tokio runtime (`main.rs`). |
| `74` | Other transport/IO failure (`EX_IOERR`). |
| `77` | `elevate` not run as root (`EX_NOPERM`). |
| `78` | `elevate` on a non-Unix host / unsupported target (`EX_CONFIG`). |

The `ClientError` enum (`error.rs`) is `#[non_exhaustive]` and covers `Usage`,
`DaemonUnreachable`, `Ipc` (from `yerd_ipc::IpcError`), `Platform` and
`Fingerprint`.

## `elevate`: the privileged orchestrator

`yerd elevate` is run via `sudo` and is the one command that does not map to a
single IPC request. Its design follows a strict trust model (see
[Elevation & Privileges](../../guide/elevation)):

- It runs as root **only to orchestrate.** It fetches read-only facts
  (`Request::DaemonInfo` → `Response::Info`: DNS address, TLD, CA path +
  fingerprint, bound HTTP/HTTPS ports) from the *invoking user's* daemon, then
  spawns the audited `yerd-helper` once per target. The daemon itself is never
  restarted as root.
- Under `sudo` the process env points at root, so the user's socket is
  reconstructed from `SUDO_UID` (uid-based, home-independent):
  `/run/user/<uid>/yerd/yerd.sock` then `/tmp/yerd-<uid>/yerd.sock`.
- The `yerdd` and `yerd-helper` binaries are derived from `yerd`'s own trusted
  `current_exe` siblings - never from the daemon - so a forged daemon cannot
  point root's `setcap` at an arbitrary binary.
- Before trusting the CA pem, the path (the only one taken from the daemon) is
  owner-checked against the invoking uid and rejected if group/world-writable.
- The helper is spawned with `env_clear()` and re-validates every argument
  independently.

`plan_invocation` is a pure function mapping `(ElevateTarget, undo)` to a
`yerd_platform::HelperInvocation`, and it is cfg-gated per OS:

| Target | macOS | Linux |
| --- | --- | --- |
| `trust` | `InstallCa` / `UninstallCa` | same |
| `resolver` | `InstallResolver` / `UninstallResolver` | same |
| `ports` | `InstallPortRedirect` / `UninstallPortRedirect` (a `pf` redirect 80→http, 443→https; reversible) | `Setcap` (grants `cap_net_bind_service` to `yerdd`); **no clean reverse**, so `unelevate ports` prints `setcap -r` guidance |

Helper exit codes are classified: `0` is success, `78` (`EX_CONFIG`) is treated
as a *skip* ("unsupported on this host", e.g. resolver without
`systemd-resolved`), and anything else is a failure that flips the run's exit
code to `1`.

## Tests and invariants

The crate is tested at two levels.

**Unit tests in `map.rs`** assert the pure invariants directly:

- `maps_each_command_to_its_request` - one assertion per command arm, including
  the `use` arity split, `--available` winning over `--check`, and `restart php`
  (specific) vs. `restart php` with no version (`RestartAllPhp`).
- `rejects_bad_version_and_name_before_connect` - bad names, versions and PHP
  settings produce `ClientError::Usage` with no I/O.
- Rendering tests pin the human output and, importantly,
  `renders_doctor_and_sets_exit_code_on_fail` /
  `json_rendering_is_valid_and_codes_match` verify the JSON and human paths
  produce the **same** exit code.

**`tests/cli_e2e.rs`** boots a real daemon on a tempdir (only the IPC task; no
proxy/DNS, since no shipped command touches them) and drives every command
through `map::to_request` + `transport::exchange_at`. It exercises `park` →
`sites`, `link`/`unlink`, per-site `use`, the `secure`/`unsecure` toggle,
`status`, `doctor` (asserting the `NoPhpInstalled` FAIL renders as exit `1`),
`list parked`, and the canonical-path `unpark` round-trip (including idempotent
re-`unpark`). This is the test that justifies the lib/bin split in `lib.rs`:
binary-only crates expose no Rust API to integration tests, so the modules are
published as a library.

## See also

- [CLI Reference](../../reference/cli/) - full command catalogue and examples.
- [yerdd (daemon)](./yerdd) - the server the CLI talks to.
- [yerd-helper (privileged)](./yerd-helper) - what `elevate` spawns.
- [IPC Protocol](../ipc-protocol) - the framing and `Request`/`Response` types.
- [Elevation & Privileges](../../guide/elevation) - the user-facing trust model.
