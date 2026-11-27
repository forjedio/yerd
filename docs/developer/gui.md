# Desktop App Internals

The Yerd desktop app (`apps/yerd-gui`) is a **Tauri v2** application: a Rust
bridge wrapping a system webview that renders a **Vue 3 + TypeScript + Tailwind**
frontend. Its single architectural rule, shared with the CLI, is that it is a
**thin `yerd-ipc` client of the `yerdd` daemon** - it contains no business logic
of its own and **never runs as root**. Everything the app does to your machine
goes through the daemon over a local socket, or through the audited `yerd` CLI
under OS elevation.

For the user-facing tour of the app, see the [Features guide](../guide/desktop-app).
This page is the contributor's reference to the two layers and the contract that
keeps them aligned.

::: info Two layers, one contract
The Rust `src-tauri` layer is a **transport bridge**: one Tauri command per
`yerd-ipc` `Request`. The Vue frontend is a **typed client**: a hand-pinned
TypeScript mirror of the same wire JSON. The whole design exists to keep both
sides agreeing with the Rust IPC contract while never duplicating daemon logic.
:::

## Module map

```
apps/yerd-gui/
├── package.json            yerd-gui (workspace version) - "a thin IPC client of the yerdd daemon"
├── vite.config.ts          Vite + Vitest (one config; vitest/config augments it)
├── tailwind.config.js      Tailwind 3 theme
├── tsconfig.json           "@/*" -> src/*
├── src-tauri/              Rust BRIDGE
│   ├── Cargo.toml          crate yerd-gui (bin yerd-gui), edition 2024, rustc ≥ 1.85
│   ├── tauri.conf.json     windows (main + dumps + mails), CSP, bundle targets
│   ├── capabilities/default.json   permission allowlist
│   └── src/
│       ├── main.rs         Builder, plugins, invoke_handler, tray, window events; show_dumps window helper
│       ├── mail_window.rs  show_mails_window command (show+focus the static `mails` window)
│       ├── commands.rs     one #[tauri::command] per Request; finish() error mapping
│       ├── ipc.rs          exchange() - socket transport, mirrors the CLI's
│       ├── elevate.rs      OS-elevated `yerd <verb> <target>` (pkexec / osascript)
│       ├── daemon.rs       locate / download-install / start / stop yerdd (host-side)
│       ├── autostart.rs    per-user service + autostart plugin + gui-settings.json
│       └── error.rs        GuiError { code, message }
└── src/                    Vue FRONTEND
    ├── main.ts             createApp + router; initTheme(); initDesktopChrome()
    ├── App.vue             AppShell + Toaster; shared daemon poller; first-run daemon start
    ├── router.ts           hash router: /overview (default) /general /php /sites /tooling /services /proxies /dumps /integrations /mail /doctor (+ /dumps-window, /mails-viewer standalone routes; a legacy route redirects to /general for the tray)
    ├── ipc/
    │   ├── types.ts        TypeScript mirror of the yerd-ipc wire JSON
    │   ├── client.ts       typed wrappers around invoke() + IpcError
    │   └── client.test.ts  command-mapping + error-categorisation tests
    ├── composables/        useDaemon (singleton poller), usePoll, useToast, useWideLayout, useIdes, useSiteEditor,
    │                       useUpdateCheck (Settings ▸ About self-update state, owned by the Settings view),
    │                       useAutoScroll (pins a streamed log box to its bottom),
    │                       useElevation (OS-privilege Fix / Fix all / Revert, shared by Welcome and Doctor),
    │                       useCreateSiteJob / useToolPrereqs / useSiteBasics and the starters
    │                       (useSiteStarters, useLaravelStarter, useWordPressStarter) behind New site
    ├── components/         AppShell, SideNav, NavLink, TitleBar, PageHeader, AboutSection, StatusPill, ComingSoon, EnvironmentCard,
    │                       SiteRoutesPanel / SiteNodePanel,
    │                       sites/ (SitesList, SiteFilterChips, SitesWarningStrip, SitesEmptyState, SiteInspector,
    │                       SiteActionsMenu, SiteEnvironmentCard, SiteDomainsCard + DomainDialog, SiteHeadersCard +
    │                       HeaderDialog, WebRootDialog, LinkFolderDialog),
    │                       proxies/ (ProxiesList, ProxyInspector, NewProxyDialog, ProxiesEmptyState),
    │                       php/ (PhpVersionsList, PhpInspector, PhpSharedDefaults, PhpSettingsGrid,
    │                       InstallPhpDialog, InstallProgressDialog),
    │                       node/ (NodeInstallDialog, NodeInstallLogDialog, NodePickOrderCard, NodeOffDefaultCard),
    │                       tooling/ (AvailableToolCard, ToolInstallLogDialog),
    │                       site-create/ (NewSiteDialog + ProjectStep, StarterPicker, LaravelOptionsStep,
    │                       WordPressOptionsStep, MissingToolsPanel, CreateProgress),
    │                       services/ (ServicesList, ServiceInspector, ServiceDatabasesCard, ServiceConnectCard,
    │                       ServiceOverridesCard, AddServiceDialog, ServiceLogsDialog, ServiceIcon),
    │                       mail/ (MailInbox, MailReader, MailSetupDialog),
    │                       dumps/ (DumpsRequestList, DumpsRequestDetail, DumpsEventCard, DumpsCapturePill,
    │                       DumpsSetupDialog),
    │                       share/ (ShareProviderCards, ShareList, ShareInspector, ShareSiteDialog,
    │                       CloudflareLoginDialog, providers/ (TunnelProvidersDialog,
    │                       CloudflarePane, NgrokPane, ZrokPane, PaneHeader, BinarySection, ProviderTile)),
    │                       doctor/ (DoctorList, DoctorInspector),
    │                       settings/ (SettingsGroup, SettingRow, UpdateActions, ConnectAgentDialog, GuiLogsDialog,
    │                       DiagnosticsDialog, MadeByForjed),
    │                       PhpVersionPanel, AddExtensionModal, DaemonDownHero, DaemonDiagnosticsPanel,
    │                       overview/ (status strip, sites card with Pinned and Recently added tabs and its OverviewSiteRow,
    │                       services, setup checklist, release notes and contributors cards),
    │                       ui/ (incl. AsyncState, EmptyState, InfoBanner, ConfirmDialog, DialogFrame,
    │                       SegmentedControl, SuffixInput, FilterChips, InspectorFrame, InspectorFooter (teleported
    │                       into the frame's pinned bar below its scroll area),
    │                       styles.ts with the shared CARD / INSPECTOR_TOOL / DANGER_TOOL / SECTION_LABEL / LIST_LABEL classes)
    ├── views/              OverviewView, GeneralView, PhpView, NodeView, SitesView, ProxiesView, ToolingView, ServicesView, LaravelDumpsView, DumpsWindowView, MailView, MailsViewerView, DoctorView
    └── lib/                utils (cn, plural / joinWithAnd, humanisers, CHIP_TONE and the run-state dot), theme,
                            desktop chrome, domainValidation,
                            toastActions (copy-to-clipboard and fire-and-forget actions that toast the outcome),
                            overview (status, health-row, pinned, recently added, services, contributors and release-notes
                            derivations for the Overview),
                            sites (Sites list sections, chips, warnings and inspector labels),
                            proxies (Proxies chips, sections, upstream labels and name/prefix checks),
                            services (Services sections, status lines, test-copy grouping, .env snippets),
                            mail (app labels, time labels, capture state, Laravel .env),
                            dumps (merging dump pages, grouping by request, event tags and bodies,
                            capture pill, follow-newest selection), dumpLog,
                            share (Share sections, state labels, facts copy, tunnel chip),
                            elevation (OS-privilege rows, the single source for the Doctor dot),
                            doctor (Doctor finding groups, default selection, permission copy),
                            settings (Settings banners, the update headline, socket-address ports),
                            php (PHP list order, status and update labels, banner, install choices
                            and the install-progress line),
                            tooling (Tooling split, descriptions, Composer gate, PATH copy),
                            createSite (the New site command preview and durations),
                            siteFilter (match a query against a site's domains and path),
                            ideChoice (resolve "auto"/"system"/an id to a label), wpAdmin,
                            phpSettings (client-side pool/setting shape checks), windowState,
                            windowZoom (double-click-to-zoom shared by TitleBar and PageHeader)
```

## The Rust bridge (`src-tauri`)

The bridge is a small, deliberately logic-free crate. Its
[`Cargo.toml`](https://github.com/forjedio/yerd) depends on the same internal
crates the CLI uses - `yerd-core`, `yerd-ipc` (with the `transport` feature),
and `yerd-platform` - because the GUI is "a client of the same contract the CLI
uses." It bans `unwrap`/`expect`/`panic` in its own code via Clippy lints;
`unsafe` is allowed only for one documented `geteuid` FFI call in `elevate.rs`.

### Commands: one per `Request`

`commands.rs` is the heart of the bridge. Each daemon-backed Tauri command maps
`command → Request`, calls [`ipc::exchange`](#the-transport-exchange), and runs
the result through `finish`, which converts a `Response::Error` into a typed
`GuiError` so the frontend only ever sees a success variant or a typed failure:

```rust
/// Convert a daemon `Response::Error` into a `GuiError`; pass success through.
fn finish(resp: Response) -> Result<Response, GuiError> {
    if let Response::Error { code, message } = &resp {
        return Err(GuiError::daemon(code_str(code), message.clone()));
    }
    Ok(resp)
}
```

The error code is rendered to its snake_case wire string **via serde**, not a
hand-written match - so a new `ErrorCode` variant needs no change here:

```rust
fn code_str(code: &ErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "internal".to_owned())
}
```

A representative command shows the pattern - no logic, just `Request`
construction and `finish`:

```rust
#[tauri::command]
pub async fn link(name: String, path: String) -> Result<Response, GuiError> {
    finish(exchange(&Request::Link { name, path: PathBuf::from(path) }).await?)
}
```

The main command groups registered via `main.rs`'s `tauri::generate_handler!` (the full list - ~50 commands spanning sites, PHP, services, databases, dumps, mail, tools, site-creation jobs, and the host-only daemon/autostart/CLI-on-PATH commands - lives in `main.rs`):

| Command | `Request` | Notes |
| --- | --- | --- |
| `ping` | `Ping` | liveness |
| `list_sites` | `ListSites` | |
| `park` | `Park { path }` | path wrapped as `PathBuf` |
| `link` | `Link { name, path }` | |
| `unlink` | `Unlink { name }` | |
| `list_parked` | `ListParked` | |
| `unpark` | `Unpark { path }` | path sent **verbatim as `String`** (matched exactly, not canonicalised, so a deleted folder is still removable) |
| `set_php` | `SetPhp { name, version }` | |
| `set_secure` | `SetSecure { name, secure }` | |
| `set_web_root` | `SetWebRoot { name, path }` | `path: Option<String>`; `null` = reset to auto-detect |
| `list_php` | `ListPhp` | |
| `check_php_updates` | `CheckPhpUpdates` | |
| `available_php` | `AvailablePhp` | |
| `install_php` | `InstallPhp { version }` | |
| `set_default_php` | `SetDefaultPhp { version }` | |
| `update_php` | `UpdatePhp { version }` | `version: Option<PhpVersion>`; `None` = update all |
| `set_php_settings` | `SetPhpSettings { settings }` | `BTreeMap<String, String>` |
| `set_xdebug` | `SetXdebug { version, enabled, modes, start }` | `version: Option<PhpVersion>` (`None` = default PHP); `modes: Option<Vec<XdebugMode>>`, `start: Option<XdebugStart>` keep the current value when `None`; replies `PhpVersions` |
| `restart_php` | `RestartPhp { version }` | |
| `restart_all_php` | `RestartAllPhp` | |
| `uninstall_php` | `UninstallPhp { version }` | |
| `restart_daemon` | `RestartDaemon` | |
| `status` | `Status` | |
| `diagnose` | `Diagnose` | |
| `doctor_fix` | `DoctorFix` | |
| `daemon_info` | `DaemonInfo` | |

Three commands are **host-only helpers** with no daemon IPC:

| Command | Returns | Purpose |
| --- | --- | --- |
| `protocol_version` | `u32` (`yerd_ipc::PROTOCOL_VERSION`) | the negotiated IPC protocol version, for the Settings page |
| `host_platform` | `&'static str` (`std::env::consts::OS`) | `"linux"` / `"macos"` / `"windows"` to gate platform UI |
| `elevate` / `unelevate` | `()` | run `yerd elevate <target>` / `yerd unelevate <target>` under OS elevation (see below) |

The Settings page (route `/general`) adds further host-only commands (no daemon IPC) for daemon lifecycle and autostart - `daemon_installed`, `start_daemon`, `stop_daemon`, `cli_path_status`, `install_cli_to_path`, `remove_cli_from_path`, `open_login_items`, `get_autostart`, `set_autostart_daemon`, `set_autostart_gui`, `set_gui_minimized`, `daemon_self_repair_busy` - implemented in `daemon.rs` (resolve the bundled binaries, start/stop, install the `yerd` CLI on PATH) and `autostart.rs` (per-user service + run-at-login; macOS uses `smappservice.rs`). The daemon is **bundled** in the app, so there's no download/install command.

On macOS, `autostart.rs` also self-repairs the daemon's SMAppService registration
on every launch (`ensure_daemon_registration`, run from a background thread in
`setup_app`). It folds the GUI/daemon version
comparison (`decide`) and a phantom-job check into a `RegAction`: `Refuse` when
the registered daemon is newer than this GUI (a downgrade guard - the running
daemon is left alone and the version conflict is surfaced on the Overview
banner); `NoOp` when the registration already matches this GUI and the launchd
job is healthy; `Reregister` (unregister, `register_repairing`, kickstart)
either when the version needs to reconcile, or when the version is already
up to date but SMAppService reports the registration `.enabled` while
`launchctl print` shows no job for it - a **phantom registration** (a BTM
hiccup, a crash, or a manual `bootout`) that a plain kickstart can't recover
from since there's no job to kick. Every branch logs to
`{cache}/yerd-gui-repair.log`, which `daemon_diagnostics`/"Copy diagnostics"
surfaces - the first place to check when a daemon doesn't come back after an
update.

Because this thread runs unconditionally on every launch - including when
onboarding is still showing (a prior "Install" click can leave a registration
behind even if the journey wasn't finished) - it can be doing real,
multi-second work (or opening Login Items) with no user click involved. The
thread brackets its call to `ensure_daemon_registration` with a
`DAEMON_SELF_REPAIR_BUSY` atomic flag and a `daemon-self-repair` Tauri event
(`true` before, `false` after), which `useDaemonStart.ts`'s `backgroundBusy`
signal (below) ORs into the Install/Start button's busy state, so the button
never sits idle while this is happening. `DAEMON_REG_LOCK` is what actually
serializes registration/kickstart calls against a concurrent manual start; the
flag/event are purely a UX signal layered on top, not a correctness mechanism.

::: tip No `Request` is ever built in the frontend
The `Request` enum is intentionally **not** mirrored into TypeScript. The
frontend never constructs raw requests; it invokes named commands and the
bridge builds the `Request`. Only `Response` (and the domain types it carries)
crosses to the webview.
:::

### The transport: `exchange`

`ipc.rs` is "a near-verbatim mirror of `bin/yerd/src/transport.rs`." It resolves
the socket path identically to the daemon and CLI - `<runtime>/yerd.sock`, where
`<runtime>` comes from `yerd_platform`'s `ActivePaths::resolve` - so client and
server always agree on the location:

```rust
#[cfg(unix)]
pub async fn exchange(req: &Request) -> Result<Response, GuiError> {
    use yerd_platform::{ActivePaths, Paths};
    let dirs = ActivePaths::new()
        .resolve()
        .map_err(|e| GuiError::unreachable(format!("cannot resolve runtime dir: {e}")))?;
    exchange_at(&dirs.runtime.join("yerd.sock"), req).await
}
```

`exchange_at` connects with `interprocess` local sockets, writes a single framed
request with `yerd_ipc::write_message` (bounded by `DEFAULT_MAX_FRAME`), and
reads one framed `Response` back with a `FrameDecoder`. It is factored out so
tests can target a tempdir socket. Failure handling distinguishes categories:

- a connect/resolve failure becomes `GuiError::unreachable(..)` - this is what
  flips the frontend's "Daemon not running" state;
- a read/write failure becomes `GuiError::internal(..)`;
- the daemon closing the connection without replying becomes an `unreachable`
  error.

On non-Unix targets `exchange` is a stub returning an `unreachable` error,
because the Windows named-pipe name is not client-derivable yet - the GUI is
macOS/Linux-only for the same reason the CLI transport is. See the
[IPC Protocol](./ipc-protocol) and [Cross-Platform Model](./cross-platform) pages.

### `GuiError` and the wire shape

`error.rs` defines the one error type every command returns. It carries only a
machine-readable `code` and a human `message`, and serialises **manually** so
the wire shape is exactly `{ code, message }`:

```rust
pub struct GuiError {
    /// daemon `ErrorCode` (snake_case), `"unreachable"`, or `"internal"`.
    pub code: String,
    pub message: String,
}
```

The three constructors - `unreachable`, `internal`, `daemon(code, message)` -
are the only categories the frontend's `IpcError` needs to distinguish.

### App lifecycle, plugins, and the tray (`main.rs`)

`main.rs` wires the Tauri `Builder`:

- **`tauri-plugin-single-instance` is registered first**: a second launch shows
  and focuses the existing `main` window instead of spawning a duplicate (which
  would risk a duplicate daemon connection or tray).
- **Launch-time update check** (`tray::spawn_launch_update_check`): fired from
  both `setup_app` and the single-instance callback, so it runs on a cold start
  and on every re-invoke of an already-running app. It asks the daemon for the
  cached self-update status and, only if `yerd_update::is_check_due` says the
  last check is stale (the same 4h threshold the daemon's own background poll
  uses), kicks a bounded live check. Silent and non-blocking - an unreachable
  daemon or fetch failure is swallowed exactly like the daemon's own
  failure-tolerant polling.
- `tauri-plugin-opener` and `tauri-plugin-dialog` back the host helpers
  (`openInBrowser`, `openPath`, `pickDirectory`).
- **Close-to-tray**: `WindowEvent::CloseRequested` hides the window and calls
  `api.prevent_close()`. The tray's **Quit** item is the real exit; **Open Yerd**
  reshows the window. (On Linux AppIndicator, clicks aren't delivered, so the
  menu item is the only way in.)
- **Dynamic tray menu** (`tray.rs`): the menu is rebuilt from live daemon state,
  not static. A Rust-side poll over the same `yerd-ipc` socket the commands use
  (the frontend poller pauses when the window is hidden to tray) diffs a snapshot
  and `set_menu`s a fresh menu only on change - showing daemon status + ports,
  Start/Restart/Stop, an inline default-PHP switcher, update items, the Mail/Dumps
  viewers, site actions, and nav shortcuts. A `TRANSITION`/`MENU_LOCK` guard keeps
  a tray-initiated lifecycle action's transient menu from being stomped by a
  late poll tick. The same poll composites status badges onto the tray icon: a red
  dot for a waiting update and an orange dot for unread mail (`draw_dot`). Menu
  item icons are recoloured per-build for the desktop's dark/light state
  (`menu_icon`/`dark_menu_bar`), since `muda`'s menu icons aren't templates and
  won't auto-tint themselves: on macOS this reads the `AppleInterfaceStyle` user
  default (unchanged); on Linux it probes the xdg-desktop-portal `Settings`
  `color-scheme` preference via the `dark-light` crate, a bounded D-Bus round
  trip taken by the caller *before* `MENU_LOCK` is acquired (see the module doc
  comment) so it never runs lock-held. Any read failure (no portal, older
  desktop) falls back to light, i.e. today's black glyphs.
- On **Linux**, before GTK initialises, `glib::set_prgname("yerd-gui")` pins the
  Wayland `app_id` so the dock matches `yerd-gui.desktop`, and a `with_webview`
  block clamps WebKitGTK's zoom level (the only place that can intercept
  Ctrl+wheel / pinch zoom, which WebKit handles below the DOM). Even earlier,
  as the first line of `main()` (before logging or any thread starts),
  `linux_gfx::apply` sets `__NV_DISABLE_EXPLICIT_SYNC=1` when the session is
  Wayland and the `nvidia_drm` module is loaded, unless the user already set it.
  Without it, WebKitGTK's dmabuf renderer on the NVIDIA driver makes the
  compositor drop the connection (`Error 71 (Protocol error) dispatching to
  Wayland display`) and the window flashes and exits. macOS needs no
  equivalent, so the module is Linux-only.

The window itself (`tauri.conf.json`) is **decorationless and transparent**
(`"decorations": false`, `"transparent": true`, `macOSPrivateApi: true`), which
is why the frontend ships a custom `TitleBar.vue`. Its control layout isn't
hardcoded to the host OS - it's a user preference. `lib/titleBarStyle.ts` holds
a `TitleBarStyle` (`"auto" | "macos" | "linux" | "windows"`),
persisted host-side in `gui-settings.json` via the `get_title_bar_style` /
`set_title_bar_style` commands (mirroring the tray icon variant setting) and
broadcast to every open window so the main, mails, and dumps windows switch in
lockstep. `"auto"` (the default) resolves from `host_platform`; any other value
forces that layout regardless of the actual host, driving which controls
`TitleBar.vue` renders (macOS traffic lights; Linux close/minimize/maximize;
Windows minimize/maximize/close). Every style renders on the left: the main
window draws them at the top of the sidebar (`TitleBar variant="sidebar"`,
whose slot holds the brand block), and the secondary windows in their own
32px bar. A settings file written by an older build with the removed mirrored
Linux style still loads as `"linux"` (a serde alias). In the main window each page's
`PageHeader` toolbar is the other drag region, sharing the double-click zoom
in `lib/windowZoom.ts`. The CSP is locked down to
`default-src 'self'` (plus inline styles and `data:` images). Bundle targets are
`deb` and `app` - the macOS `.dmg` is **not** a Tauri bundle target; it's built
as a separate headless step (`apps/yerd-gui/scripts/build-macos-dmg.sh`, via
`appdmg`) after the `.app` is signed, since Tauri's own dmg bundler drives
Finder via AppleScript to style the background/icon layout, which isn't
reliable outside an interactive session. See
[Packaging and releasing](./building#packaging-and-releasing). For `.deb`, the
privileged-port capability is **not**
baked into the artifact: the `postinst` script grants
`cap_net_bind_service=+ep` on the installed `yerdd` at configure time (and
re-applies it on every upgrade, since dpkg wipes file capabilities) - falling
back to ports 8080/8443 if `setcap` is missing or the filesystem can't hold
caps. There's no AppImage target because its ephemeral mount can't host a
`postinst` step, so the daemon's `setcap` can't be persisted that way. The native
Arch package (`.pkg.tar.zst`) is **not** a Tauri bundle target (Tauri has no
pacman bundler) - it's built separately from
[`packaging/arch/PKGBUILD`](https://github.com/forjedio/yerd/blob/main/packaging/arch)
in the release workflow's `arch` job, with a `.install` scriptlet doing the same
`setcap`. The Fedora `.rpm`, by contrast, **is** a Tauri bundle target (Tauri v2
has an rpm bundler): the `fedora` jobs run `tauri build` with the
`tauri.bundle-linux-rpm.conf.json` overlay and an `rpm/postinst.sh` `%post` doing
the same `setcap`. See [Packaging and releasing](./building#the-fedora-package-rpm).

::: info Three windows, one bundle
The app is no longer single-window. `tauri.conf.json` declares **three** windows,
all loading the same SPA bundle at different hash routes and all hidden until
shown:

- **`main`** - the app shell (`index.html`, default route).
- **`mails`** - the standalone Mails viewer (`#/mails-viewer`), declared
  statically.
- **`dumps`** - the live Laravel Dumps viewer (`#/dumps-window`). It is also
  declared statically, but `main.rs`'s `show_dumps` helper *lazily (re)creates* it
  if it has been destroyed, so opening it from the Dumps page is robust either way.

The auxiliary windows are **shown, not spawned**: `mail_window::show_mails_window`
and `show_dumps_window` just `get_webview_window(label)` then `show()` + focus.
When a window isn't already open, the shared `reveal_aux_window` helper first
centres it on the monitor under the cursor (the active screen), so it appears
where the user is looking rather than on whatever display it last lived on.
The `CloseRequested` handler is **global** (fires for every window) and hides
rather than closes each one, so the windows persist across opens. Crucially it
gates the close-to-tray + Dock-accessory behaviour on `window.label() == "main"`:
closing an auxiliary window must not yank the main app's Dock presence or
minimise the whole app to the tray. On the frontend side, `App.vue` detects the
auxiliary windows (`getCurrentWindow().label === "dumps"`, or a route with
`meta.standalone`) and renders the bare viewer with **no SideNav/TitleBar shell
and no daemon poller**, so an auxiliary window never runs a second `status` loop.
The dynamic tray menu opens both auxiliary windows directly (its **Mail** and
**Dumps** items call the same reveal helpers).
:::

### Capabilities

`capabilities/default.json` is the permission allowlist. Our own
`#[tauri::command]`s are permitted by registration; the file additionally grants
`core:default`, the `opener:default` / `dialog:default` plugin commands, and the
specific `core:window:*` commands the custom titlebar drives
(`start-dragging`, `minimize`, `maximize`/`unmaximize`/`toggle-maximize`,
`is-maximized`, `close`).

## In-app elevation

`elevate.rs` is the only privileged path, and it is careful. The GUI process
never becomes root; instead it **elevates the audited `yerd` CLI** to run one of
a fixed allowlist of verbs against a fixed allowlist of targets:

```rust
const TARGETS: [&str; 3] = ["trust", "resolver", "ports"];
const VERBS: [&str; 2] = ["elevate", "unelevate"];
```

`run(verb, target)` rejects anything not in those allowlists (both are
interpolated into the macOS AppleScript, so both must be validated), resolves the
trusted CLI, and spawns the blocking, prompt-driven process off the async runtime
via `spawn_blocking`. The `elevate` command passes `"elevate"`; `unelevate` (the
Services-tab "Unelevate" buttons) passes `"unelevate"`. The
invariants (grounded in `bin/yerd/src/elevate.rs`) are:

1. **Elevate the CLI, not the GUI.**
2. **Resolve `yerd` as a sibling of our own `current_exe`**, never from `PATH`
   or the daemon - an anti-forgery measure matching the CLI. If no `yerd` sits
   beside the app binary, the command fails with an explanatory error.
3. **Thread the real uid through `env SUDO_UID=<uid>`** because the elevation
   tool clears the environment, and `yerd elevate` relies on `SUDO_UID` to
   locate the user's socket and owner-check the CA.

Per platform:

```sh
# Linux (<verb> is elevate or unelevate)
pkexec /usr/bin/env SUDO_UID=<uid> <yerd> <verb> <target>

# macOS - built on osascript's stdin, with `quoted form of` for shell safety
osascript:  do shell script "env SUDO_UID=<uid> " & quoted form of "<yerd>" \
            & " <verb> <target>" with administrator privileges
```

The macOS branch reads `SUDO_UID` from `libc::geteuid()` and embeds it because
`osascript … with administrator privileges` runs as root with a clean env and
does **not** set `SUDO_UID` (that is a `sudo`-ism). Cancellation is detected from
exit codes (`pkexec` 126/127) or stderr text (`User canceled` / `-128`), since on
macOS the exit code alone can't separate "dismissed" from "elevate failed". On
unsupported platforms the helper returns an error telling the user to run
`yerd elevate` in a terminal. See [Elevation & Privileges](../guide/elevation).

## The Vue frontend (`src`)

### The typed IPC client

`ipc/types.ts` is "a *contract*, pinned by hand to the Rust source." It mirrors
the `yerd-ipc` wire JSON and documents each type's origin so review catches
drift; the file's header names `crates/yerd-ipc/src/{request,response,status}.rs`
and `crates/yerd-core/src/site.rs` as the source of truth, with
`yerd-ipc`'s `tests/wire_stability.rs` guarding the Rust side. Wire conventions:
enums are internally tagged on `type`, `snake_case`; `PhpVersion` is the bare
string `"8.5"`; `Option<T>` is `T | null`. It mirrors the additive `SiteEntry`
domain fields (`primary_domain?`, `domains?`, `apex_shadowed_by?`) and the
`DomainShadow` type carried in `StatusReport.shadows`.

`Response` is the central discriminated union:

```ts
export type Response =
  | { type: "pong" }
  | { type: "sites"; sites: Site[] }
  | { type: "ok" }
  | { type: "error"; code: ErrorCode; message: string }
  | { type: "parked"; paths: string[] }
  | { type: "info"; dns_addr: string; tld: string; ca_path: string; ca_fingerprint: string }
  | { type: "php_versions"; installed: PhpVersion[]; default: PhpVersion;
      updates?: PhpUpdate[]; settings?: Record<string, string> }
  | { type: "available_php"; available: PhpVersion[]; installed: PhpVersion[] }
  | { type: "status"; report: StatusReport }
  | { type: "diagnoses"; items: Diagnosis[] }
  | { type: "doctor_fix"; report: FixReport };
```

`ipc/client.ts` wraps each Tauri command in a typed function and narrows the
`Response` for callers - including the domain mutators
`addDomain` / `removeDomain` / `setPrimaryDomain` / `resetDomains` that back
`sites/SiteDomainsCard.vue`, the routing mutators `addRouteRule` / `removeRouteRule` /
`listRoutes` behind `SiteRoutesPanel.vue`, the header mutators
`setResponseHeader` / `removeResponseHeader` / `listResponseHeaders` behind
`sites/SiteHeadersCard.vue`, `nodeSite` / `setNodeVersion` / `clearNodeVersion`
behind `SiteNodePanel.vue`, `listNode` / `availableNode` /
`installNodeWithProgress` / `uninstallNode` behind `NodeView.vue`,
`setPhpPoolSettings` behind
`PhpVersionPanel.vue`, and `serviceOverrides` / `setServiceOverride` behind
`services/ServiceOverridesCard.vue`. A low-level `call` normalises every rejection into an
`IpcError`, and `ensureOk` defensively throws if a `type:"error"` ever slips
through:

```ts
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try { return await tauriInvoke<T>(cmd, args); }
  catch (e) { throw toIpcError(e); }
}
```

`IpcError` categorises failures so the UI can react. Its `unreachable` flag is
set when the code is `"unreachable"` **or** the message matches a
daemon-down pattern - that flag is what drives the global "Daemon not running"
card:

```ts
this.unreachable =
  code === "unreachable" || /daemon (is )?unreachable|not running/i.test(message);
```

The client also exposes host helpers that are **Tauri plugins, not daemon IPC**:
`openInBrowser` / `openPath` (opener plugin), `pickDirectory` (dialog plugin),
and `elevate` (the `elevate` command above), typed to the
`ElevateTarget = "trust" | "resolver" | "ports"` union.

A second group of host-only commands backs the **editor and terminal** launchers
in the Sites inspector, implemented over
[`yerd-platform`](./crates/yerd-platform)'s `IdeLauncher` / `SystemOpener` /
`TerminalLauncher` seams rather than any daemon request:

| Wrapper | Command | Purpose |
| --- | --- | --- |
| `getInstalledIdes` | `get_installed_ides` | rank-sorted detected editors (id + display label) |
| `openInIde` | `open_in_ide` | open a site's folder in the editor with this id |
| `openInSystemDefault` | `open_in_default` | open it with the desktop's default handler |
| `openInTerminal` | `open_terminal` | open a terminal with that directory as its cwd |
| `getPreferredIde` / `setPreferredIde` | `get_preferred_ide` / `set_preferred_ide` | the app-wide preference (`null` = auto-detect) |
| `getSiteIdeOverrides` / `setSiteIdeOverride` | `get_site_ide_overrides` / `set_site_ide_override` | per-site overrides of that preference |

`open_in_ide` / `open_in_default` take a **site name**, not a path: the host
resolves the directory from the daemon's site list, so no absolute path is
handed back and forth across the webview boundary. The editor preference and the
per-site overrides persist host-side in `gui-settings.json`, alongside the tray
icon variant and title-bar style. Preferences are stored as **raw ids**, so an id
this host can't currently see (an uninstalled editor, or one set on another
machine) survives untouched and simply renders as auto-detect - `lib/ideChoice.ts`
does that resolution.

### Composables

| Composable | Role |
| --- | --- |
| `useDaemon` | **Singleton** daemon store. One poller for the whole app (connection pill, views) so the daemon isn't hit by N independent `status` loops. `status` doubles as the liveness probe; only a genuine **unreachable** error flips `connected` to `false` - a typed daemon error still means the daemon is up. Started/stopped from `App.vue`. |
| `useDaemonStart` | **Singleton** "start the daemon, wait for it to connect, diagnose on failure" flow shared by onboarding step 1, `DaemonDownHero`, and Doctor. `phase` (click-driven, fed by the Rust `daemon-start-phase` event) and `backgroundBusy` (fed by macOS's launch-time self-repair thread's `daemon-self-repair` event, see above) both OR into `starting`/`activeLabel` so every consumer's button reflects either kind of in-flight work - but `start()`'s own re-entrancy guard reads `phase` alone, so a same-tick self-repair no-op can never suppress `App.vue`'s automatic start. |
| `usePoll` | Generic mount-scoped poller. Never overlaps in-flight calls, **pauses while the document is hidden** (background tab / tray), refreshes on becoming visible, and clears its timer on unmount. Default cadence 4s; callers should not go below ~3s for `status`. |
| `useToast` | Module-level toast store rendered by the single `<Toaster>` in `App.vue`. Errors linger (8s), success/info auto-dismiss (4s). |
| `useIdes` | **Singleton** cache of the host's detected editors, in rank order. Detection is a filesystem scan, so it runs once per session (`loadIdes`) and every editor picker reads the same list rather than re-probing on open; `rescanIdes` backs the Settings **Rescan** button. Each run carries a monotonic generation token and publishes only while it is still the newest, so a slow initial load can never overwrite a rescan the user triggered after it. |
| `useUpdateCheck` | The About section's self-update flow (channel, cached and live checks, apply). `GeneralView` owns one instance and passes it to whichever `AboutSection` layout is showing, so crossing the wide breakpoint mid-check keeps the spinner and the result; a stale or superseded check never overwrites a newer one. |

Both pollers gate on `document.visibilityState === "hidden"` to avoid hammering
the daemon when the window is hidden to the tray - a real cost, since each
`status` reads the trust store and live FPM state.

### Views, components, and `lib`

The hash router (`createWebHashHistory`, because the webview loads from a
file/asset origin) maps the in-shell routes - **OverviewView** (`/overview`, the
default landing dashboard: quick actions, status, pinned and recently added sites, services, contributors and release notes), **GeneralView** (`/general`, the **Settings** page),
**PhpView**, **NodeView** (`/node`, installed releases, an Install dialog picking from the upstream releases list,
streamed installs, Update, uninstall, and the global default; in the command
palette without a digit), **SitesView**, **ToolingView** (`/tooling`; Node is
not listed there, it has its own page), **ServicesView**,
**LaravelDumpsView** (`/dumps`), **MailView** (`/mail`), **DoctorView** (findings
plus the OS-privilege rows - CA trust / resolver / ports - via `useElevation`),
and **IntegrationsView** (`/integrations`, Share). `/about` redirects to
`/general` (the tray's About and Update items still navigate there; the build
info, updates, links and troubleshooting live in `AboutSection.vue`, the right
column of Settings in a wide window and its first card in a narrow one). Plus two **standalone** routes that the auxiliary windows load:

- **DumpsWindowView** (`/dumps-window`) and **LaravelDumpsView** (the in-shell
  `/dumps` page) both compose `components/dumps/` (`DumpsRequestList`,
  `DumpsRequestDetail`, `DumpsEventCard`, `DumpsCapturePill`) over the
  `useDumps` composable, which pages the daemon's ring incrementally with
  `listDumps(since)` (reconciled by `mergePage` in `lib/dumps.ts`), polls
  `dumpsStatus`, groups events by request, follows the newest request, and
  drives `setDumpsEnabled` / `setDumpsPersist` / `setDumpFeature` /
  `clearDumps`. The page adds `DumpsSetupDialog` (interception, signals,
  per-PHP-version extension presence, a link to the port in Settings) and an
  open-in-window button (`showDumpsWindow`); the window adds always-on-top and
  keeps streaming while hidden.
- **MailsViewerView** (`/mails-viewer`, `meta.standalone`) and **MailView**
  (the in-shell `/mail` page) both compose `components/mail/` (`MailInbox`,
  `MailReader`) over the `useMailbox` composable, which polls `listMails`, keeps
  the app filter (grouped by sender name, then the attributed site), loads the
  selected message with `getMail` (a user's pick is marked read with
  `markMailsRead`; the automatic first pick is not) and deletes or clears.
  `MailReader` renders the HTML body, sanitized with DOMPurify, into a **Shadow
  DOM** (remote content blocked until **Load**, except local resources on the site
  TLD, `localhost` or loopback, which always load; see `lib/mailLinks.ts`), with Text and Headers tabs and
  attachments. `MailView` adds the toolbar: the capture pill from the shared
  `status` poll, **Setup** (`MailSetupDialog`, driving `setMailEnabled`; the port
  is edited in Settings ▸ Application Ports) and a pop-out button that opens the
  viewer window via `showMailsWindow`.

Components are split into app components (`AppShell`, `SideNav`, `TitleBar`,
`StatusPill`, `PageHeader`, `ComingSoon`) and hand-rolled
**shadcn-vue-style `ui/` primitives** (`Button`, `Card`, `Input`, `Select`,
`Switch`, `Modal`, `Spinner`, `Toaster`, `Badge`, plus `dropdown-menu/`,
`tabs/` and `tooltip/` built on `reka-ui`). `lib/utils.ts` holds the shadcn `cn` helper and
the display humanisers - note `poolStateLabel` and `runStateDotClass`, which render an
installed-but-not-serving FPM pool as **"idle"** (neutral) rather than the
alarming wire value `stopped`, reserving red **"failed"** for an actual crash.
`Button`, `Input` and `Select` take `class` as a prop merged through `cn`, so a
caller's height, size, padding or font-size utilities replace the variant
defaults instead of racing them in CSS order.

### "Coming soon" affordances

`ComingSoon.vue` renders a deliberately non-interactive `<span>`
(`aria-disabled="true"`, a native `title` tooltip, no clickable element) so a
gated control reads as intentional rather than broken. Its only use today: on a
platform without in-app elevation - i.e. a future Windows build, since the GUI
ships only on macOS/Linux - the *Fix* action on the Doctor page's **System
permissions** (and Welcome's environment card) falls back to a "soon" pill
pointing at `yerd elevate`. On the
supported platforms every control is fully wired (the earlier Logs / restart
stubs are gone now that their IPC exists).

::: warning The GUI is a client of the daemon's state
When the socket is unreachable, `AppShell.vue` replaces the route view with a
"Daemon not running" panel offering **Start** (which launches `yerdd` through
the per-user service via the `start_daemon` host command) and **Retry**; the
Overview and Settings routes stay reachable while down (`DAEMON_FREE`).
The app can drive the daemon's *lifecycle* but never reimplements its runtime
logic - the daemon stays the single source of truth - and it expects the `yerd`
CLI installed **beside** it for the elevation path.
:::

## Testing and the type-check gate

The frontend is tested with **Vitest** (jsdom environment, configured in the
single `vite.config.ts` via `vitest/config`):

- `ipc/client.test.ts` mocks `@tauri-apps/api/core`'s `invoke` and asserts the
  **command-mapping contract** (e.g. `listSites` unwraps the array,
  `updatePhp(null)` sends `{ version: null }`), that a daemon `Response::Error`
  becomes an `IpcError` carrying its `code`, and the `IpcError` unreachable
  categorisation.
- `components/components.test.ts` mounts components with `@vue/test-utils`
  (e.g. `ComingSoon` is non-interactive; `StatusPill` tri-state tones).
- `lib/utils.test.ts` covers the humanisers.

```sh
npm run test       # vitest run
npm run build      # vue-tsc --noEmit && vite build  - type-check is part of build
npm run typecheck  # vue-tsc --noEmit standalone
```

`npm run build` runs **`vue-tsc --noEmit`** before `vite build`, so a frontend
type error - including drift between `ipc/types.ts` and how the views consume
`Response` - **fails the build**. The Rust bridge has its own unit tests
(`finish` passes success through, maps a daemon error to the right code,
`code_str` renders snake_case for known variants):

```sh
cargo test -p yerd-gui    # needs the Linux -dev packages listed in the README
```

Together, the Vitest command-mapping tests, the `vue-tsc` gate, the bridge unit
tests, and `yerd-ipc`'s own `wire_stability` tests are what keep the
TypeScript contract pinned to the Rust contract on both sides.
