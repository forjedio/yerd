# Phase 4 Implementation Plan — Privilege & elevation: UAC helper, NRPT wildcard DNS, uninstall

Working doc (not committed). Companion to `WINDOWS_PLAN.md` Phase 4 and successor to
`PHASE3_PLAN.md` (Phases 1–3 committed as `ed9add2` / `1e0de5a` / `ed12af6`). Everything
below was verified against the actual code on this Windows machine on 2026-08-03,
including a source-level audit of `runas 1.2.0` and `is_elevated 0.1.2` in the local
cargo registry.

Locked decisions honoured throughout: **`.test` DNS = NRPT wildcard** (single
namespace-wide rule), **elevation model = per-op UAC helper** (`ShellExecuteEx` +
`runas` verb launching `yerd-helper.exe`, NOT an admin manifest).

---

## 0. Ground truth (verified, not assumed)

### 0.1 The helper contract as it exists today

- `HelperInvocation` (`crates/yerd-platform/src/helper.rs`) already has
  `InstallResolver { tld, addr }` / `UninstallResolver { tld }` variants with a frozen
  argv shape (`install-resolver --tld test --addr 127.0.0.1:53`), round-trip tested
  (`tests/helper_argv_shape.rs`, helper `tests/argv_contract.rs`).
  **Phase 4 needs NO change to `HelperInvocation` or its argv shape** — the Windows
  NRPT op reuses the existing resolver variants verbatim.
- `bin/yerd-helper/src/main.rs:29-33`: on Windows `main` is a stub returning exit 78
  before touching dispatch; `#![cfg_attr(not(any(linux, macos)), allow(dead_code,
  unused_imports))]` at lines 15-18 silences the resulting dead code. Both go away.
- `privilege.rs`: Linux reads `/proc/self/status`; **macOS shells out to
  `/usr/bin/id -u` by absolute path** — an explicit no-`unsafe` precedent for a
  subprocess-based privilege probe at the security boundary. The
  `not(any(linux, macos))` arm returns `None` (never privileged).
- Exit-code contract (`bin/yerd-helper/src/error.rs::exit_code`): 64 usage, 65
  validation/data, 69 tool-not-found, 70 wire drift, 74 io, 75 command failed,
  77 not privileged, 78 unsupported. `bin/yerd/src/elevate.rs::run_one` and
  `uninstall.rs::run_helper` already interpret 0/65/78/other. Unchanged.
- Helper subprocess precedent: despite the "never shell out" hard rule in
  `yerd-helper.instructions.md`, the **Linux resolver op already runs
  `systemctl`/`nmcli`/`dnsmasq`** through `ops/mod.rs::run_command` (env-cleared,
  pinned `PATH`). The rule in practice means "no `sh -c`, no PATH-trusting, no
  doing-the-work-in-a-shell-string with untrusted input"; a pinned-absolute-path
  tool invocation with validated typed args is the established pattern. The Windows
  NRPT op follows it (see §2.4).
- The debug-build clap ↔ `from_argv` cross-check (`cli.rs:137-154`) filters
  `--skip-priv-check` before calling `from_argv`. The new Windows-only
  `--result-token` flag (§2.5) must be filtered the same way (flag **and** its value).

### 0.2 The elevation call sites as they exist today

- `bin/yerd/src/elevate.rs` `windows_impl` (Phase 3): `Trust` is real (direct
  CurrentUser-Root mutation, no helper, no admin — that stays exactly as is);
  `Resolver` prints "arrives with Phase 4" and skips; `Ports`/`Lan` are permanent
  Windows skips (direct bind). Phase 4 replaces only the `Resolver` arm.
- The Unix flow's shape to preserve: fetch `Facts` from the daemon
  (`Request::DaemonInfo` for `dns_addr`/`tld`, `Request::Status` for
  `report.dns_unbound` health), refuse install when DNS is unbound/unhealthy, spawn
  the sibling helper with `HelperInvocation::to_argv()`, classify by exit code.
  `transport::exchange` already works on Windows (the Phase 3 `run_trust` uses it).
- `require_user_owned` (elevate.rs:549) exists **only** to stop root from trusting a
  CA PEM file the invoking user doesn't own before `InstallCa` runs privileged. On
  Windows, CA trust is unelevated (CurrentUser store, Phase 3) and the only elevated
  Phase 4 op (`install-resolver`/`uninstall-resolver`) **takes no file path at all**
  — there is no file for the elevated path to be tricked into trusting. See §4.3.
- `bin/yerd/src/uninstall.rs`: `#[cfg(not(unix))] run()` declines with exit 78.
  The Unix flow captures `tld` + CA fingerprint from disk **before** deleting
  anything, then drives the helper directly (no daemon needed). The Windows path
  mirrors this. `PlatformDirs::for_user` already has a correct Windows arm (Phase 1
  fix, verified at `paths.rs:88-100`), but on Windows the uninstalling CLI runs as
  the invoking user (no sudo env distortion), so `ActivePaths::resolve()` is the
  primary source and `for_user` is not needed here (see §5).
- GUI: `apps/yerd-gui/src-tauri/src/elevate.rs` elevates the CLI via
  pkexec/osascript; Windows returns an explanatory error and the frontend gates the
  "Fix" button off Windows. Because on Windows `yerd elevate` is itself unelevated
  (the helper prompts UAC per-op), the GUI needs no elevation at all — it can spawn
  the sibling CLI normally. **Deliberately out of Phase 4 scope** (master plan puts
  frontend work in Phase 6); tracked in §7.

### 0.3 The DNS server really does answer arbitrary `*.test` (confirmed)

`crates/yerd-dns/src/responder.rs::answer` is purely suffix-based: any name ending
in `.<tld>` (any depth, any label, case-insensitive, no registration lookup) returns
`Answer::Loopback4` for A / `Loopback6` for AAAA — the table test covers
`xapp.test`, `a.b.c.app.test`. Names outside the TLD return `Refused` with AA
cleared, apex returns `NoData`, malformed labels `NxDomain`. `server.rs::Bound`
binds **UDP + TCP on the same port** (hickory `ServerFuture`), which is what the
Windows stub resolver will hit. **NRPT can safely hand the whole namespace to it.**

### 0.4 NEW FINDING — NRPT carries no port; the Windows DNS default must become 53

NRPT rules (`-NameServers` / the `GenericDNSServers` registry value) accept **IP
addresses only, no port**. Queries go to `<ip>:53`. But
`yerd-config::DEFAULT_DNS_PORT` is **1053** (`schema.rs:345`). Unlike macOS
`/etc/resolver` files (which have a `port` line) and systemd-resolved
(`DNS=ip:port`), NRPT cannot express 1053.

Consequence: on Windows the bundled resolver must listen on `127.0.0.1:53`.
Binding 53 is **unprivileged on Windows** (no <1024 restriction — same fact that
made Phase 3's direct 80/443 bind work), and loopback listeners raise no firewall
prompt. Conflict risk (Acrylic/AdGuard/Technitium users; ICS binds its adapter IP,
not loopback; Docker Desktop/WSL2 don't take host loopback 53) is handled by the
**already-existing** `dns_unbound` machinery: the daemon reports the failed bind,
`yerd elevate resolver` refuses with the port + remedy, doctor flags it.

Enforced at three layers (§2.2, §3, §2.4): Windows config default 53; the elevate
preflight refuses a non-53 `dns_addr`; the helper independently validates
`--addr` = loopback IPv4 port 53 before writing the rule (defence in depth — a rule
pointing at a server that isn't on 53 silently blackholes `.test`).

### 0.5 Registry crates already in the dependency graph

`Cargo.lock` already resolves `winreg` (0.10.1 / 0.52.0 / 0.55.0) and
`windows-registry 0.2.0` transitively (tauri chain). Adding `winreg = "0.55"` as a
workspace dep re-uses an existing resolution — no new transitive surface. `runas` and
`is_elevated` are **not** currently in the lock; `runas 1.2.0` adds itself plus
`which 4.3` (unconditional dep in its manifest) — `which` is new to the lock; its
`windows-sys 0.48` requirement already resolves.

---

## 1. FFI-touchpoint determinations (the security audit)

Each Win32 touchpoint, in the same style as Phase 2's `win32job` and Phase 3's
`schannel` audits. Workspace `unsafe_code = "forbid"` stays intact everywhere: **no
forbid-lift is required anywhere in Phase 4.** Full options/rationale in §8.

| Touchpoint | Mechanism chosen | `unsafe` in our code | New dep |
|---|---|---|---|
| Launch helper elevated (UAC) | `runas 1.2.0` crate (`ShellExecuteExW` + `runas` verb inside the crate) | No | `runas` (+`which`, transitive) in `bin/yerd`, cfg(windows), pinned |
| Token-elevation check (helper + CLI) | `whoami.exe /groups` subprocess by absolute path, integrity-level SID parse (mirrors macOS `id -u` + Phase 1 `spawn_whoami_sid` precedents) | No | none |
| Owner-SID / ACL check | **Not needed** — no Windows helper op consumes a file path (§4.3); tracked TODO | No | none |
| NRPT install/remove (elevated, HKLM) | `powershell.exe` DnsClient cmdlets by pinned absolute path, inside the elevated helper | No | none |
| NRPT presence probe (unprivileged) | `winreg` read-only of `HKLM\...\Dnscache\Parameters\DnsPolicyConfig` | No (crate wraps it, like schannel/win32job) | `winreg 0.55` in `yerd-platform`, cfg(windows) |
| Result-file handshake | plain `std::fs` in the user's runtime dir | No | none |

### 1.1 `runas 1.2.0` audit (read in full, `src/impl_windows.rs`, 90 lines)

Does exactly what Phase 4 needs: `ShellExecuteExW` with verb `runas`,
`SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS`, `WaitForSingleObject(INFINITE)`,
`GetExitCodeProcess` → returns `io::Result<ExitStatus>`. Blocks the calling thread
(call under `tokio::task::spawn_blocking`, like Phase 3's trust dialog). Three
quirks that shape our usage (all mitigated by design, none disqualifying):

1. **Arg quoting is buggy for quoted args containing backslashes.** Args needing
   quoting (space/tab/quote) get every `\` doubled, which `CommandLineToArgvW`
   reads as literal double backslashes — a path like
   `C:\Users\John Smith\...\result.txt` would arrive corrupted. Args that need no
   quoting are passed verbatim and are safe. **Mitigation: never pass an argument
   that can contain a space or backslash.** Every helper arg on Windows is from a
   closed charset: op tag, `--tld` (`yerd_core::Tld`: `[a-z0-9.-]`), `--addr`
   (`SocketAddr` display), `--result-token` (`[0-9a-f]{32}`, §2.5 — a token, not a
   path, precisely because of this). Guard test in §6.
2. **Failure conflation:** a declined UAC prompt (`ERROR_CANCELLED`) or a
   `GetExitCodeProcess` failure both surface as exit code `0xFFFFFFFF`
   (`code() == Some(-1)`), not `Err`. Map `Some(-1)`/`None` to "the elevation
   prompt was declined or the helper failed to launch".
3. `to_string_lossy()` on args — irrelevant given (1)'s ASCII-only argv discipline.

Alternatives rejected: hand-rolled `ShellExecuteExW` (needs a `forbid` lift in
`bin/yerd` — not acceptable without escalation, and unnecessary);
`powershell Start-Process -Verb RunAs -Wait` (two nested quoting layers, slower,
harder to audit than 90 lines of crate source).

### 1.2 `is_elevated 0.1.2` audit vs. `whoami /groups`

`is_elevated` is a correct ~40-line `OpenProcessToken` +
`GetTokenInformation(TokenElevation)` wrapper — but it pulls `winapi` into the
**security-boundary binary** and is unmaintained (2019). The subprocess
alternative — `%SystemRoot%\System32\whoami.exe /groups /fo csv /nh` and look for
the High (`S-1-16-12288`) or System (`S-1-16-16384`) mandatory-integrity SID — is
the exact pattern this codebase already ships twice (macOS helper `id -u`;
Phase 1 `spawn_whoami_sid` in `os/windows.rs`, absolute path, never `PATH`).
A UAC-elevated process always runs at High integrity; with UAC disabled an admin
process is also High; a non-elevated process is Medium — the proxy is faithful to
`TokenElevation` for every case Yerd meets. **Chosen: `whoami /groups`,
implemented once in `yerd-platform` with a table-tested pure parser, reused by the
helper and the CLI.** (`is_elevated` documented as the fallback if the proxy ever
proves insufficient.)

### 1.3 NRPT: PowerShell cmdlets (write) + registry read (probe)

Two candidate write mechanisms were compared:

- **Raw `HKLM\SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig`
  writes** (via winreg): fast and subprocess-free, but the DNS client service does
  **not reliably observe** raw key writes — the documented pickup is a Dnscache
  restart, and Dnscache is a protected service that cannot be stopped/restarted on
  modern Windows 10/11. Rules written raw may not take effect until reboot.
- **`Add-/Remove-/Get-DnsClientNrptRule`** (DnsClient module, ships with every
  Windows 8+ client SKU): goes through the WMI/MI provider which notifies the
  service — rules apply **immediately, no reboot, no flush** (a
  `Clear-DnsClientCache` is appended anyway to drop cached negative answers).
  Cost: ~1-2 s powershell startup — irrelevant for a once-per-machine elevated op.

**Chosen (USER DECISION): FIXED-ARG DISCRETE CMDLET CALLS — no `-Command` script
body, no `Where-Object`/pipeline.** Inside the elevated helper, each powershell
invocation runs exactly ONE cmdlet with discrete fixed argv tokens, using the pinned
absolute path `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe` with
`-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command <one cmdlet>` where the
single cmdlet's parameters are discrete literal tokens (never a multi-statement
script). winreg read-only is used for BOTH the unprivileged `is_installed` probe AND
to discover existing rule GUIDs for removal (HKLM reads need no admin; the probe runs
on every daemon `Status`, so a powershell spawn there is not acceptable). This keeps
the helper's shell-out surface as close as possible to the existing fixed-argv
systemctl/nmcli precedent. Note the master plan (and locked-decision #2) says
`Set-DnsClientNrptRule`; the actual creating cmdlet is `Add-DnsClientNrptRule`
(Set- modifies an existing rule).

Discrete-call sequence (each a SEPARATE `run_command_abs("powershell", ...)` call;
no composed script):

1. **Discover** existing `.test` rule GUIDs by reading
   `HKLM\SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig`
   subkeys via winreg (already the `is_installed` mechanism) and matching the
   `Name` value against the tld. GUIDs come from OUR registry read (trusted,
   charset-safe), never from user input.
2. **Remove** each discovered rule with its own call:
   `Add/Remove-DnsClientNrptRule` params as discrete tokens →
   `["-NoProfile","-NonInteractive","-ExecutionPolicy","Bypass","-Command",
   "Remove-DnsClientNrptRule -Name '<guid>' -Force"]` (one cmdlet, fixed args; the
   exact `-Name` GUID shape is pinned by the Step-0 spike).
3. **Add** the rule: one call, discrete tokens →
   `Add-DnsClientNrptRule -Namespace '.test' -NameServers '127.0.0.1' -Comment 'yerd'`.
4. **Flush**: one call → `Clear-DnsClientCache`.

The only interpolated values are the `Tld`-validated (`[a-z0-9.-]`) tld, the
loopback IP literal, and registry-sourced GUIDs — injection-free by charset; the
argv-building functions are pure and table-tested and reject quotes defensively.

(remove-then-add = idempotent install/repair: a stale or wrong-server `.test` rule
is replaced; running twice converges). Uninstall is the first and third lines only
(no rules → no-op → idempotent). Probe: enumerate `DnsPolicyConfig` subkeys, match
one whose `Name` (REG_MULTI_SZ) contains `.<tld>` and whose `GenericDNSServers`
equals the expected IP — matching logic is a pure function fed by the winreg read.
**Implementation-time verification (do first, §2.0):** confirm on this machine the
exact value names the cmdlet writes (`Name`, `GenericDNSServers`, `ConfigOptions`,
`Version`) before freezing the probe's matcher.

---

## 2. File-by-file implementation checklist (ordered; workspace compiles after every step)

### Step 0 — machine verification spike (no code committed)

Run once, elevated, on this machine; record results as comments in `pure/nrpt.rs`:

- `Add-DnsClientNrptRule -Namespace '.test' -NameServers '127.0.0.1' -Comment 'yerd'`
  → dump `HKLM\SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig`
  and note exact value names/types; confirm `Get-DnsClientNrptRule` works
  **unelevated**; confirm `Resolve-DnsName anything.test` hits 127.0.0.1
  immediately (no reboot) once something listens on `127.0.0.1:53`; confirm
  `Remove-DnsClientNrptRule -Force` + `Clear-DnsClientCache` restores NXDOMAIN.
- Confirm `powershell.exe` runs under `Command::env_clear()` plus a minimal
  re-added env (`SystemRoot`, `windir`, `TEMP`); if it doesn't, the helper's
  Windows `run_command` keeps the parent env (it launched from a UAC-elevated
  clean context anyway) — decide from observation, note the decision.
- Confirm an unprivileged `TcpListener::bind("127.0.0.1:53")` +
  `UdpSocket::bind` succeed (no admin, no firewall prompt).

### Step 1 — `yerd-platform`: pure helpers (compiles + tests on all OSes)

New files, un-gated like `pure/win_pipe.rs` so Linux/macOS CI exercises them:

- `crates/yerd-platform/src/pure/nrpt.rs`
  - `add_rule_cmd(tld: &Tld, ip: Ipv4Addr) -> String` — the SINGLE-cmdlet
    `Add-DnsClientNrptRule -Namespace '<tld>' -NameServers '<ip>' -Comment 'yerd'`
    body for one discrete `-Command` call (one cmdlet, no pipeline/script).
  - `remove_rule_cmd(guid: &str) -> String` — the single-cmdlet
    `Remove-DnsClientNrptRule -Name '<guid>' -Force` body (guid from our winreg read).
  - `flush_cmd() -> &'static str` — `Clear-DnsClientCache`.
    All defensive: debug-assert/filter no `'` in inputs (charset already excludes it).
  - `rule_matches(name_entries: &[String], servers: &str, tld: &str, ip: &str) -> bool`
    — the probe's matcher over one registry rule's `Name` multi-sz +
    `GenericDNSServers`.
- `crates/yerd-platform/src/pure/win_token.rs`
  - `csv_has_elevated_integrity(whoami_groups_csv: &str) -> bool` — finds
    `S-1-16-12288` / `S-1-16-16384` in the SID column (substring on the quoted CSV
    is fine; SIDs are locale-independent).
- `crates/yerd-platform/src/pure/helper_result.rs` — the result-file protocol shared
  by both binaries (§2.5): `TOKEN_LEN = 32`,
  `valid_token(&str) -> bool` (`[0-9a-f]{32}`),
  `result_file_name(token) -> String` (`helper-result-<token>.txt`),
  `render(outcome) -> String` / `parse(&str) -> Option<HelperResult>` for the
  one-line `ok` / `error: <detail>` body.
- Register in `pure/mod.rs`; table tests in each file.

### Step 2 — `yerd-platform`: real `WindowsResolverInstaller` + elevation probe

`crates/yerd-platform/src/os/windows.rs` (one change, alias flipped in the same
commit — the "never half-flip" rule):

- Remove `UnsupportedResolverInstaller as WindowsResolverInstaller` from the
  `pub use super::unsupported::{...}` re-export block; add a real
  `WindowsResolverInstaller` implementing `ResolverInstaller`:
  - `install`/`uninstall` → `Err(PlatformError::NeedsHelper { operation:
    ops::INSTALL_RESOLVER / UNINSTALL_RESOLVER })` — same contract as Linux/macOS
    ("the OS impls never spawn the helper themselves").
  - `is_installed(tld, addr)` → winreg: open
    `HKLM\SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig`
    read-only, enumerate subkeys, feed each rule's values to `pure::nrpt::
    rule_matches` against `tld` + `addr.ip()`. Key absent → `Ok(false)`
    (idempotent-probe contract in `resolver.rs` trait docs). Also return `false`
    when `addr.port() != 53` (a rule can never point at a non-53 server — this is
    what makes doctor's "resolver installed" honest, mirroring the trait's
    "stale file aimed elsewhere must report false" requirement).
- `pub fn is_token_elevated() -> bool` — spawn `whoami_path()` (existing fn)
  `/groups /fo csv /nh`, parse via `pure::win_token`; failure → `false`
  (conservative, like the Linux `/proc` fallback). Export through `os/mod.rs`
  `active` alongside `current_user_sid` and re-export from `lib.rs` (cfg(windows)).
- `crates/yerd-platform/Cargo.toml`: add under `[target.'cfg(windows)'.dependencies]`
  `winreg = { workspace = true }`; root `Cargo.toml` `[workspace.dependencies]`
  gains `winreg = "0.55"` with a comment in the win32job/schannel style (read-only
  NRPT probe; unsafe inside the crate, none in ours; already resolved in
  Cargo.lock via the tauri chain).
- Trait doc touch-up in `resolver.rs` ("always return `NeedsHelper` in Phase 1" →
  note Windows too; probe reads registry).
- Gate: `cargo check -p yerd-platform` + smoke test (§6) green; Linux/macOS
  untouched (`os/linux.rs`/`macos.rs` unchanged; `unsupported.rs` unchanged and
  still total).

### Step 3 — `yerd-config`: Windows DNS default = 53

`crates/yerd-config/src/schema.rs`: split the const —
`#[cfg(windows)] pub const DEFAULT_DNS_PORT: u16 = 53;` (+ doc: NRPT cannot carry a
port) / `#[cfg(not(windows))] ... = 1053;`. Adjust any default-shape test that
hard-codes 1053 to use the const or cfg the expectation (check
`parse.rs`/`serialize.rs` byte-shape tests). Pure const — no I/O added to the pure
crate. Existing Windows dev configs that persisted 1053 keep it; the elevate
preflight (Step 5) and doctor tell them the remedy (`yerd doctor` /
`set dns_port 53` + restart). Gate: `cargo test -p yerd-config`.

### Step 4 — `bin/yerd-helper`: enable on Windows (the boundary itself)

- `main.rs`: change both cfg gates from `any(target_os = "linux", target_os =
  "macos")` to `any(target_os = "linux", target_os = "macos", windows)`; narrow the
  stub + the `allow(dead_code)` attr to `not(any(...windows))`. Gate the
  `set_current_dir("/")` line `#[cfg(unix)]` (ShellExecuteEx already starts the
  elevated child in system32; nothing meaningful to chdir to). After dispatch,
  Windows-only: write the result file (§2.5) — best-effort, never changes the exit
  code.
- `privilege.rs`: add `#[cfg(windows)] pub fn is_privileged() -> bool {
  yerd_platform::is_token_elevated() }` (the helper already depends on
  `yerd-platform`; single audited implementation, table-tested parser). Keep the
  `not(any(linux, macos))` `effective_uid` stub for other OSes but exclude windows
  from it. The existing `NotPrivileged` error/77 mapping is reused unchanged (its
  display string mentions euid — reword to "not running privileged (elevated
  token / effective uid required)" so it is OS-neutral).
- `cli.rs`: add a **Windows-only, transport-level** global arg:
  `#[cfg(windows)] #[arg(long, global = true, hide = true, value_name = "HEX")]
  pub result_token: Option<String>` on `Cli`, surfaced through `ParsedCli`.
  Validate with `pure::helper_result::valid_token` (reject → `ArgvUsage`).
  **Deliberately NOT part of `HelperInvocation`/`from_argv`** — it does not
  describe the operation; the debug cross-check's argv filter learns to drop
  `--result-token` **and its following value** (mirroring the `--skip-priv-check`
  filter). This is the only helper-contract touch in Phase 4 and it is additive +
  cfg(windows); `tests/argv_contract.rs` and `helper_argv_shape.rs` byte shapes are
  untouched.
- `ops/mod.rs`: `run_command` is Unix-`PATH`-pinned; add a
  `#[cfg(windows)] run_command_abs(tool, absolute_program, args)` variant (absolute
  `PathBuf` program, no `PATH` trust; env per Step 0's observation) and cfg the
  existing one `#[cfg(unix)]`. `atomic_write` is already `#[cfg(unix)]` — not
  needed on Windows (no files written).
- `ops/resolver.rs`: change the `not(any(linux, macos))` stubs to
  `not(any(linux, macos, windows))`; add `#[cfg(windows)]` impls:
  - `install_resolver(tld, addr)`: `validate::require_valid_tld` (existing);
    **new validation** `require_loopback_53(addr)` — IPv4 loopback (normalise an
    unspecified 0.0.0.0 from LAN-mode facts to 127.0.0.1) and `port == 53`, else
    `Validation` (65) with a new `ValidationReason::ResolverAddrUnsupported`
    variant (windows-gated, like the macOS-gated `PortInvalid`);
    then run the DISCRETE-CALL SEQUENCE (each its own `run_command_abs` spawn, one
    cmdlet per call, no composed script): winreg-discover existing `.test` GUIDs →
    one `run_command_abs("powershell", powershell_path(), [...,"-Command",
    pure::nrpt::remove_rule_cmd(guid)])` per GUID → one call with
    `pure::nrpt::add_rule_cmd(...)` → one call with `pure::nrpt::flush_cmd()`.
    `powershell_path()` derives from `%SystemRoot%` exactly like `whoami_path()`.
    The Windows `run_command_abs` must retain `SystemRoot` + `PSModulePath` in the
    otherwise-scrubbed env so the DnsClient module autoloads (M3).
  - `uninstall_resolver(tld)`: winreg-discover `.test` GUIDs → one discrete
    `remove_rule_cmd(guid)` call each → `flush_cmd()`. Idempotent by construction
    (no rules found → no-op success).
- `exec.rs` needs no change (dispatch is total; resolver arms now do real work on
  Windows, every other op's windows/`not(any)` stub still maps to `Unsupported`/78
  — correct: CA trust is unelevated on Windows, setcap/pf don't exist).
- Gate: `cargo test -p yerd-helper` on Windows; on Linux/macOS unchanged
  behaviour (`cargo check` cross-confirm via CI).

### Step 5 — `bin/yerd`: the UAC elevation flow (`elevate.rs` windows_impl)

- `bin/yerd/Cargo.toml`: `[target.'cfg(windows)'.dependencies] runas = { workspace
  = true }`; root workspace dep `runas = "=1.2.0"` **exact-pinned** with a comment
  documenting §1.1 (the quoting limitation is why argv must stay space/backslash
  free; a version bump must re-audit `impl_windows.rs`).
- `windows_impl` reworks the `Resolver` arm (Trust/Ports/Lan arms untouched):
  1. `fetch_resolver_facts()` — `Request::DaemonInfo` → `dns_addr`, `tld`;
     `Request::Status` → `report.dns_unbound` (+ health error), mirroring the Unix
     `fetch_facts(needs_dns_health)` shape and its refusal messages.
  2. Preflight (pure, unit-tested): refuse when `dns_unbound` (same message as
     Unix, naming the port); refuse when `dns_addr.port() != 53` with the remedy
     "set dns_port to 53 (`yerd doctor` explains) and restart Yerd — Windows NRPT
     can only target port 53"; normalise unspecified IP → 127.0.0.1.
  3. Build `HelperInvocation::InstallResolver/UninstallResolver` (unchanged
     contract), generate a 16-byte hex token, argv =
     `inv.to_argv()` + `["--result-token", token]`.
  4. `spawn_helper_elevated(helper_exe, argv)` in `spawn_blocking`:
     `runas::Command::new(helper).args(...).show(false).status()`. Print
     "Windows will ask for administrator approval (UAC)…" first — and skip the
     warning when `yerd_platform::is_token_elevated()` already (no prompt appears
     for an elevated console; this is the CLI-side probe the master plan asks
     for, also reusable by future preflights).
  5. Classify: `Some(0)` ok; `Some(65)` refused (same text as Unix); `Some(77)`
     "the helper did not receive an elevated token"; `Some(-1)`/`None` "the
     elevation prompt was declined or the helper failed to launch"; other codes
     as today. Then best-effort read + delete
     `<runtime>\helper-result-<token>.txt` (path from `ActivePaths::resolve()`,
     same user ⇒ same `%TEMP%` as the elevated helper) and print the `error:`
     detail line if the exit was non-zero — the file is **advisory only**, the
     exit code stays authoritative (§2.5).
  6. Windows `sibling_binaries()`: `current_exe().parent().join("yerd-helper.exe")`
     (mirror the Unix fn; no `yerdd` needed — no setcap analogue).
- Success copy: "any https://<name>.test now resolves — no per-site setup".
- Gate: `cargo test -p yerd` on Windows; Unix module untouched.

#### §2.5 Result-file protocol (designed small)

ShellExecuteEx yields no stdio, so: **exit code = the contract (unchanged
sysexits mapping); one advisory text file = the detail.**

- Parent generates `token = hex(16 random bytes)` and passes `--result-token`.
- Helper (Windows only), after dispatch: resolve its own runtime dir via
  `ActivePaths::resolve()` (elevation keeps the same user profile ⇒ identical
  `%TEMP%\yerd`), `create_dir_all`, write
  `helper-result-<token>.txt` containing exactly one line — `ok` or
  `error: <HelperError Display>` — using `File::create_new` (refuse to follow a
  pre-planted file). Write failures are swallowed (stderr note only).
- Parent reads (if present), prints the detail on failure, deletes the file.
  Missing file + non-zero exit ⇒ generic message; missing file + exit 0 ⇒ success
  (file is not load-bearing).
- Why a token and not a path: (a) the runas quoting bug (§1.1) makes
  backslash+space paths unsafe to pass; (b) a hex token cannot traverse or name a
  reparse point, so the elevated helper never opens an attacker-influenced path.
  Residual accepted risk (documented in code): another same-user process could
  junction `%TEMP%\yerd` itself; the write is a fixed-name, fixed-content,
  `create_new` text file — no meaningful primitive. Full ACL hardening of the
  runtime dir is the §7 tracked TODO.

### Step 6 — `bin/yerd`: Windows uninstall (`uninstall.rs`)

Change the stub gate to `not(any(unix, windows))`; add `windows_impl`, mirroring
the Unix order without the sudo/actor machinery (the CLI *is* the user):

1. Dirs from `ActivePaths::resolve()` (env is the invoking user's — no `for_user`
   needed on Windows; keep the Unix `for_user` caller untouched). Note in a doc
   comment why this differs from Unix (`SUDO_UID` reconstruction).
2. `CapturedFacts::capture(&dirs)` — reuse as-is (reads `yerd.toml` tld +
   `ca.cert.pem` fingerprint before anything is deleted).
3. `print_header` + `confirm()` (shared shapes; reuse/move the Unix fns to the
   outer module where trivially shareable, otherwise duplicate the small ones —
   prefer moving `confirm`/`dedup` up rather than copying).
4. Revert system state:
   - **NRPT**: if `facts.tld` present, spawn the sibling helper **elevated** via
     the Step 5 `spawn_helper_elevated` with `UninstallResolver` (one UAC prompt;
     print that it's coming). Declined/failed → residue line with the manual
     remedy: `Get-DnsClientNrptRule | Where-Object { $_.Namespace -contains
     '.test' } | Remove-DnsClientNrptRule -Force` (as admin).
   - **CA**: `WindowsTrustStore::new().uninstall_system(&fp)` directly (Phase 3
     path; per-cert confirmation dialog; no admin). No fingerprint on disk →
     residue line pointing at `certmgr.msc` → Trusted Root (Current User) →
     "Yerd Local CA".
5. Stop the daemon: no SIGTERM analogue and no service yet (Phase 5) —
   `taskkill /F /IM yerdd.exe` via absolute `%SystemRoot%\System32\taskkill.exe`
   (unelevated taskkill can only kill same-user processes, which is the intent),
   bounded wait, then proceed; Phase 2's `KILL_ON_JOB_CLOSE` job objects reap the
   php-cgi/DB children when the daemon dies. Residue note if it wouldn't die.
   (Graceful daemon-led shutdown arrives with the Phase 5 service stop — accepted
   MVP gap, mirrors the master plan's "service/autostart cleanup joins Phase 5".)
6. Delete dirs: `dedup([config, data, state, cache, runtime])` with retry-once on
   sharing violations (Defender/file-lock flakiness, Phase 2 precedent). PATH
   block / shims / binaries: **not present on Windows until Phase 5/6** — print a
   note that `yerd.exe` itself must be deleted manually (a running exe can't
   unlink itself on Windows; Phase 6's NSIS uninstaller owns that).
7. `print_summary(residue)` (reuse).
- Gate: `cargo test -p yerd`; pure helpers (dir list, remedy strings) unit-tested.

### Step 7 — `yerd-doctor`: Windows-correct remedy strings

`crates/yerd-doctor/src/lib.rs:131-139` hard-codes the remedy
`sudo yerd elevate resolver`. cfg(windows) the remedy to `yerd elevate resolver`
(no sudo; mention the UAC prompt in the detail text). Doctor's
`ResolverNotInstalled` finding otherwise lights up automatically once Step 2's
`is_installed` feeds `StatusReport.resolver_installed` (`ipc_server.rs:776-785`
already calls it through `ActiveResolverInstaller` on every status). Pure crate —
string-only change, table tests updated. Richer Windows doctor checks (NRPT rule
detail, port-53 conflict naming) stay in Phase 5 per the master plan.

### Step 8 — CI/full gate

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace` on this Windows machine; confirm the existing
ubuntu/macos legs are byte-identical in behaviour (no Unix file's logic changed —
only cfg re-gating in `uninstall.rs`/`main.rs`/`privilege.rs`/`ops` and the shared
`ValidationReason`/error-string rewords; re-run Unix-side unit tests via CI).

---

## 3. Ordering constraints

- Step 1 → 2 (impls consume the pure helpers); Step 2 → 5/6/7 (probe + alias must
  exist before callers light up); Step 4 → 5 (the CLI drives the helper; e2e-ing
  Step 5 needs a Windows-capable helper binary); Step 3 is independent but must
  land before the Step 5 preflight is *useful* (else every elevate hits the
  port-53 refusal); Step 6 depends on 4+5 (shared elevated-spawn helper).
- Every step compiles and tests green in isolation; the `active` alias flip
  (Step 2) is atomic with the trait impl per the master plan's "never half-flip"
  rule; nothing else touches `os/mod.rs`.

## 4. Scope discipline (what Phase 4 deliberately does NOT do)

1. **No `HelperInvocation`/argv-shape change, no `yerd-ipc` wire change.** The one
   contract touch is the additive, cfg(windows), transport-level `--result-token`
   clap arg on the helper (flagged in §2 Step 4).
2. **No hosts-file fallback**, no LocalMachine cert store, no service registration
   (Phase 5), no GUI/frontend change (§7).
3. **Owner-SID / ACL checks: not implemented — justified, not skipped.** The Unix
   `require_user_owned` exists to protect the *privileged CA-trust* path from
   trusting a user-substitutable file. On Windows: CA trust is unelevated +
   CurrentUser (Phase 3) and the sole elevated op takes `--tld`/`--addr` only —
   validated typed values, **no file path** — so `GetNamedSecurityInfo` has
   nothing to guard. Written as a tracked TODO (with the `secure_fs.rs`
   ACL-hardening item) in `TODO.md`, to be revisited the moment any Windows helper
   op accepts a path argument.

## 5. Tests to add (all run on the Windows CI leg; pure ones run everywhere)

| Test | Where | Kind |
|---|---|---|
| PS command composition (install/uninstall, tld variants, quote-rejection) | `pure/nrpt.rs` | pure table |
| Registry-rule matcher (match, wrong server, wrong tld, multi-namespace, empty) | `pure/nrpt.rs` | pure table |
| whoami `/groups` CSV integrity parse (High, System, Medium, garbage, empty) | `pure/win_token.rs` | pure table |
| Token validity + result filename + render/parse round-trip | `pure/helper_result.rs` | pure table |
| `is_installed` on an unconfigured machine → `Ok(false)` (read-only, CI-safe) | `yerd-platform` windows smoke | integration |
| `is_installed(port != 53)` → `Ok(false)` | windows smoke | integration |
| Helper: `--result-token` parses, bad token → usage(64); debug cross-check filter | `bin/yerd-helper` cli tests (cfg windows) | unit |
| Helper: `install-resolver` with port ≠ 53 / non-loopback → 65, **before** any spawn | helper op unit (cfg windows) | unit |
| Helper: result file written with `ok`/`error:` line, `create_new` refusal path | helper unit (temp runtime dir override or fn-level test) | unit |
| Elevate preflight: unbound DNS refused; port ≠ 53 refused with remedy; 0.0.0.0 normalised | `elevate.rs` windows_impl unit | pure unit |
| **runas argv discipline guard**: every argv element of every Windows invocation contains no space/tab/quote/backslash | `bin/yerd` unit iterating the Windows `HelperInvocation`s + `--result-token` | pure unit |
| Exit-code classification incl. `Some(-1)` → declined message | elevate windows_impl unit | pure unit |
| Uninstall dir list dedup / remedy strings | `uninstall.rs` windows tests | pure unit |
| Doctor windows remedy string has no `sudo` | `yerd-doctor` | pure table |
| Manual (documented in this file, not CI): full e2e — `yerd elevate resolver` → UAC → `Resolve-DnsName whatever.test` → 127.0.0.1 → browse `https://anything.test`; `yerd unelevate resolver`; `yerd uninstall` leaves no NRPT rule/CA | — | manual DoD |

## 6. New pinned dependencies (complete list)

| Crate | Version | Where | Why safe |
|---|---|---|---|
| `runas` | `=1.2.0` (exact pin + audit comment) | `bin/yerd`, cfg(windows) | 90-line audited `ShellExecuteExW` wrapper; unsafe inside the crate only (win32job/schannel precedent); quirks mitigated by the space-free-argv guard test. Brings `which 4.3` (transitive, unconditional in its manifest — noted, accepted). |
| `winreg` | `0.55` | `yerd-platform`, cfg(windows) | Read-only HKLM NRPT probe; already resolved in Cargo.lock; unsafe inside the crate only. |

No dep is added to `yerd-helper` (whoami/powershell are OS binaries by absolute
path; `yerd-platform` it already has). Depcheck guards
(`yerd-platform`/`yerd-helper` `no_runtime_deps.rs`) are unaffected — nothing new
in their forbidden lists' reachable sets.

## 7. Tracked TODOs handed onward (add to `TODO.md` in this phase)

- **ACL enforcement** (`secure_fs.rs` analogue): 0o600-equivalent DACLs for the CA
  key + runtime dir hardening, and an owner-SID check *if* a Windows helper op
  ever takes a path argument. MVP-accepted gap per the master plan.
- **GUI elevate on Windows** (Phase 6 frontend polish): spawn the sibling CLI
  *unelevated* (`yerd elevate resolver` — the helper raises UAC itself), un-gate
  the frontend "Fix" button for `windows`.
- Doctor Phase 5 items: NRPT-rule detail check, 127.0.0.1:53 squatter naming.
- `runas` upstream quoting bug: consider an upstream fix/PR or replacing with a
  first-party wrapper if argv ever needs paths.

## 8. ESCALATION / DECISION section

**No human decision is strictly blocking; no `unsafe` forbid-lift is needed
anywhere.** Points a reviewer should consciously sign off:

1. **UAC launch = `runas 1.2.0` crate** (unsafe internal to the crate, like
   win32job/schannel). Alternatives: (a) first-party `ShellExecuteExW` wrapper —
   requires lifting `forbid(unsafe_code)` in `bin/yerd` (rejected; escalate only
   if runas proves unusable); (b) `powershell Start-Process -Verb RunAs` (worse
   quoting surface, rejected). **Sign-off: accept the crate's known quirks under
   the space-free-argv discipline + guard test.**
2. **Privilege check = `whoami /groups` integrity-level proxy**, not literal
   `GetTokenInformation(TokenElevation)`. Faithful for every reachable case
   (UAC-elevated ⇒ High; UAC-off admin ⇒ High; otherwise Medium). Alternative:
   `is_elevated 0.1.2` (true TokenElevation, but unmaintained + pulls winapi into
   the security boundary). **Sign-off: proxy accepted; fallback documented.**
3. **NRPT mechanism = PowerShell DnsClient cmdlets (write) + winreg (read)** —
   chosen over raw `DnsPolicyConfig` registry writes because raw writes are not
   reliably observed by the unstoppable Dnscache service (reboot risk); the
   cmdlets notify it and apply immediately. This means the elevated helper spawns
   `powershell.exe` — consistent with the Linux helper's pinned
   systemctl/nmcli/dnsmasq precedent, but a reviewer of
   `yerd-helper.instructions.md`'s "never shell out" rule should approve the
   reading in §0.1 explicitly.
4. **Windows `DEFAULT_DNS_PORT` becomes 53** (new finding, §0.4 — NRPT cannot
   carry a port). Cross-platform default divergence in a pure crate; enforced/
   diagnosed at three layers. **This is the one genuine product-behaviour decision
   in the phase — flag to the user, recommend proceeding.**
5. **Owner-SID check dropped as not-load-bearing on Windows** (§4.3) rather than
   ported. Reviewer should confirm the reasoning (no elevated op consumes a path).
6. Result-file residual risk (same-user junction of `%TEMP%\yerd`) accepted for
   MVP: fixed name, fixed benign content, `create_new`, advisory-only semantics.

## 9. Definition of done (from the master plan, made concrete)

- On this machine: `yerd elevate resolver` (unelevated shell) → single UAC prompt
  → `Get-DnsClientNrptRule` shows one `.test` → 127.0.0.1 rule →
  `https://<anything>.test` resolves + loads green with **no** manual edit and no
  further prompts per site; re-running elevate converges (idempotent repair).
- `yerd-helper.exe install-resolver …` run from an *unelevated* shell exits 77
  without side effects; garbage argv exits 64/65 before any spawn.
- `yerd unelevate resolver` removes the rule; `yerd uninstall` removes rule + CA +
  dirs, leaving only `yerd.exe` (with the documented manual note).
- `cargo fmt`/`clippy -D warnings`/`test --workspace` green on Windows, ubuntu,
  macos CI; Unix elevation behaviour byte-identical; no `unsafe`, no forbid-lift,
  `HelperInvocation` argv byte-shape tests untouched and green.
