//! The system-tray icon and its **dynamic** dropdown menu.
//!
//! Unlike the rest of the GUI, the tray must stay correct while the main window
//! is closed to tray - exactly when the frontend's daemon poller pauses (it
//! short-circuits on `document.visibilityState === "hidden"`). So the tray owns a
//! small **Rust-side poll** over the same `yerd-ipc` socket the commands use,
//! rebuilding the menu only when a diffed snapshot of daemon state actually
//! changes. This stays a thin client: it only calls `yerd-ipc`, never daemon
//! logic.
//!
//! Concurrency: a tray-initiated daemon lifecycle action (start/stop/restart)
//! owns the menu for the duration of the action via the `TRANSITION` flag, and
//! `MENU_LOCK` makes the poller's "is a transition active? then apply" decision
//! atomic with respect to the action clearing the flag - so a poller tick that
//! began its fetch before the action can't overwrite the action's transient menu.
//! `MENU_LOCK` is only ever taken from spawned worker tasks, never the main
//! thread (a worker blocks on the main thread while `set_menu` marshals, so a
//! main-thread lock would cycle-deadlock).

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tauri::menu::{
    CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Wry};

use yerd_core::PhpVersion;
use yerd_ipc::{Request, Response};

use crate::ipc::{exchange, exchange_timeout};

const TRAY_ID: &str = "yerd-tray";
/// Background poll cadence. Modest because the tray is rarely open; diff-gating
/// means most ticks don't rebuild anything.
const POLL_INTERVAL: Duration = Duration::from_secs(6);
/// Bound for each tray status probe so a wedged daemon can't stall the poller.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounded wait for a daemon lifecycle action to settle (500ms × 20 ≈ 10s).
const SETTLE_STEPS: u32 = 20;
const SETTLE_STEP: Duration = Duration::from_millis(500);

/// The three navigable pages the tray links to (demoted below the direct
/// actions). PHP has its own submenu and Mail/Dumps their own openers, so they
/// aren't repeated here.
const NAV_ITEMS: &[(&str, &str)] = &[
    ("nav:/sites", "Sites"),
    ("nav:/services", "Services"),
    ("nav:/about", "About"),
];

/// macOS menu-bar template icon (see `main.rs` for the rationale).
#[cfg(target_os = "macos")]
const TRAY_ICON_MAC: &[u8] = include_bytes!("../icons/tray-mac.png");

/// Set while a tray-initiated daemon lifecycle action owns the menu.
static TRANSITION: AtomicBool = AtomicBool::new(false);
/// Serializes the poller's `check TRANSITION + set_menu` with an action's clear.
/// Held only around a synchronous `set_menu`, never across an `.await`, and never
/// from the main thread.
static MENU_LOCK: Mutex<()> = Mutex::new(());

/// Take `MENU_LOCK`, recovering a poisoned lock rather than panicking (a poisoned
/// menu lock guards no critical invariant; `.unwrap()` would trip the workspace
/// `clippy::unwrap_used` deny).
fn lock_menu() -> MutexGuard<'static, ()> {
    MENU_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A diffable snapshot of the daemon state the menu renders. Plain data only, so
/// the per-tick equality check never touches muda objects. Deliberately does NOT
/// include live uptime: it would change every tick and rebuild the menu on a
/// timer (churn, and a rebuild can disrupt the menu if the user has it open), for
/// only a cosmetic label - so the menu rebuilds solely on meaningful state change.
#[derive(Clone, Default, PartialEq, Eq)]
struct TrayState {
    running: bool,
    default_php: Option<String>,
    installed: Vec<String>,
    http: Option<u16>,
    https: Option<u16>,
    update_target: Option<String>,
}

/// Build the tray and register it; called once from `setup_app`.
pub(crate) fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app, &TrayState::default())?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Yerd")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(on_tray_icon_event);

    #[cfg(target_os = "macos")]
    {
        builder = builder
            .icon(tauri::image::Image::from_bytes(TRAY_ICON_MAC)?)
            .icon_as_template(true);
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}

/// Spawn the background poll loop; called from `setup_app` after `build_tray`.
pub(crate) fn spawn_tray_poller(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last: Option<TrayState> = None;
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        loop {
            interval.tick().await;
            // A transition owns the menu; don't even fetch-and-compare.
            if TRANSITION.load(Ordering::Acquire) {
                continue;
            }
            let state = fetch_state().await;
            if last.as_ref() == Some(&state) {
                continue;
            }
            // Re-check TRANSITION under the lock so a transition that began during
            // the fetch above wins (its transient menu is not overwritten).
            let guard = lock_menu();
            if !TRANSITION.load(Ordering::Acquire) {
                apply(&app, &state);
                last = Some(state);
            }
            drop(guard);
        }
    });
}

/// Fetch daemon status (+ cached update target) into a snapshot. An unreachable
/// daemon yields the "stopped" snapshot.
async fn fetch_state() -> TrayState {
    let report = match exchange_timeout(&Request::Status, PROBE_TIMEOUT).await {
        Ok(Response::Status { report }) => report,
        _ => return TrayState::default(),
    };
    // Cache-only, so cheap and offline-safe; only surface a target when available.
    let update_target = match exchange_timeout(&Request::CachedUpdateStatus, PROBE_TIMEOUT).await {
        Ok(Response::UpdateStatus {
            available: true,
            target,
            ..
        }) => target,
        _ => None,
    };
    TrayState {
        running: true,
        default_php: Some(report.default_php.to_string()),
        installed: report.php.iter().map(|p| p.version.to_string()).collect(),
        http: Some(report.http.bound),
        https: Some(report.https.bound),
        update_target,
    }
}

/// Build + install the menu for `state` (and the macOS title). No lock / no
/// transition logic - callers hold `MENU_LOCK` as needed.
fn apply(app: &AppHandle, state: &TrayState) {
    let Ok(menu) = build_menu(app, state) else {
        return;
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_menu(Some(menu));
        #[cfg(target_os = "macos")]
        {
            let _ = tray.set_title(Some(tray_title(state)));
        }
    }
}

/// Install the transient menu shown while a lifecycle action is in flight.
fn apply_transient(app: &AppHandle, label: &str) {
    let Ok(menu) = build_transient_menu(app, label) else {
        return;
    };
    let guard = lock_menu();
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_menu(Some(menu));
        #[cfg(target_os = "macos")]
        {
            let _ = tray.set_title(Some("…"));
        }
    }
    drop(guard);
}

/// Fetch the current state and apply it under the lock unless a transition owns
/// the menu. Used by the non-lifecycle actions (set-default-PHP, update check).
async fn refresh_now(app: &AppHandle) {
    let state = fetch_state().await;
    let guard = lock_menu();
    if !TRANSITION.load(Ordering::Acquire) {
        apply(app, &state);
    }
    drop(guard);
}

/// macOS menu-bar title: the active PHP version when running, else a stopped dot.
#[cfg(target_os = "macos")]
fn tray_title(state: &TrayState) -> String {
    if state.running {
        state.default_php.clone().unwrap_or_else(|| "●".to_string())
    } else {
        "○".to_string()
    }
}

/// Push an owned menu item as a boxed trait object (lets the builder mix
/// `MenuItem`/`CheckMenuItem`/`Submenu`/separators in one list).
fn push(items: &mut Vec<Box<dyn IsMenuItem<Wry>>>, item: impl IsMenuItem<Wry> + 'static) {
    items.push(Box::new(item));
}

fn finish_menu(app: &AppHandle, items: &[Box<dyn IsMenuItem<Wry>>]) -> tauri::Result<Menu<Wry>> {
    let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(AsRef::as_ref).collect();
    Menu::with_items(app, &refs)
}

/// The full menu for the current daemon state.
fn build_menu(app: &AppHandle, state: &TrayState) -> tauri::Result<Menu<Wry>> {
    let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();

    if state.running {
        push(
            &mut items,
            disabled(app, "noop:header", "● Daemon running")?,
        );
        if let (Some(http), Some(https)) = (state.http, state.https) {
            push(
                &mut items,
                disabled(app, "noop:ports", format!("HTTP :{http} · HTTPS :{https}"))?,
            );
        }
        match &state.update_target {
            Some(target) => push(
                &mut items,
                action(app, "update:apply", format!("Update to {target}…"))?,
            ),
            None => push(
                &mut items,
                action(app, "update:check", "Check for updates")?,
            ),
        }
        push(&mut items, action(app, "daemon:restart", "Restart daemon")?);
        push(&mut items, action(app, "daemon:stop", "Stop daemon")?);
        push(&mut items, PredefinedMenuItem::separator(app)?);

        if !state.installed.is_empty() {
            push(&mut items, build_php_submenu(app, state)?);
            push(&mut items, PredefinedMenuItem::separator(app)?);
        }

        push(&mut items, action(app, "new-site", "New Laravel site…")?);
    } else {
        push(
            &mut items,
            disabled(app, "noop:header", "○ Daemon stopped")?,
        );
        push(&mut items, action(app, "daemon:start", "Start daemon")?);
        push(&mut items, PredefinedMenuItem::separator(app)?);
    }

    push(&mut items, action(app, "open", "Open Yerd")?);
    push(&mut items, PredefinedMenuItem::separator(app)?);
    push(&mut items, action(app, "mail", "Mail")?);
    push(&mut items, action(app, "dumps", "Dumps")?);
    push(&mut items, PredefinedMenuItem::separator(app)?);
    for (id, label) in NAV_ITEMS {
        push(&mut items, action(app, id, *label)?);
    }
    push(&mut items, PredefinedMenuItem::separator(app)?);
    push(&mut items, action(app, "quit", "Quit Yerd")?);

    finish_menu(app, &items)
}

/// The collapsed menu shown during a daemon transition: a disabled status line
/// plus the always-safe actions. Crucially, **no** daemon-lifecycle items, so a
/// second start/stop/restart can't fire mid-transition and clear `TRANSITION`.
fn build_transient_menu(app: &AppHandle, label: &str) -> tauri::Result<Menu<Wry>> {
    let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();
    push(&mut items, disabled(app, "noop:transient", label)?);
    push(&mut items, PredefinedMenuItem::separator(app)?);
    push(&mut items, action(app, "open", "Open Yerd")?);
    push(&mut items, action(app, "mail", "Mail")?);
    push(&mut items, action(app, "dumps", "Dumps")?);
    push(&mut items, PredefinedMenuItem::separator(app)?);
    push(&mut items, action(app, "quit", "Quit Yerd")?);
    finish_menu(app, &items)
}

/// The "Default PHP ▸" submenu: one checkable item per installed version, the
/// current default checked.
fn build_php_submenu(app: &AppHandle, state: &TrayState) -> tauri::Result<Submenu<Wry>> {
    let title = match &state.default_php {
        Some(v) => format!("Default PHP: {v}"),
        None => "Default PHP".to_string(),
    };
    let checks: Vec<CheckMenuItem<Wry>> = state
        .installed
        .iter()
        .map(|v| {
            let checked = state.default_php.as_deref() == Some(v.as_str());
            CheckMenuItem::with_id(app, format!("php:set:{v}"), v, true, checked, None::<&str>)
        })
        .collect::<tauri::Result<_>>()?;
    let refs: Vec<&dyn IsMenuItem<Wry>> =
        checks.iter().map(|c| c as &dyn IsMenuItem<Wry>).collect();
    Submenu::with_items(app, title, true, &refs)
}

/// A clickable item.
fn action(
    app: &AppHandle,
    id: impl Into<tauri::menu::MenuId>,
    text: impl AsRef<str>,
) -> tauri::Result<MenuItem<Wry>> {
    MenuItem::with_id(app, id, text, true, None::<&str>)
}

/// A non-interactive label (status header / subline).
fn disabled(
    app: &AppHandle,
    id: impl Into<tauri::menu::MenuId>,
    text: impl AsRef<str>,
) -> tauri::Result<MenuItem<Wry>> {
    MenuItem::with_id(app, id, text, false, None::<&str>)
}

/// The single global menu-event handler. It keeps receiving events for items
/// installed later via `set_menu` (the listener is registered on the tray, not
/// the menu instance), so every dynamic id is matched here. Runs on the main
/// thread; it must never take `MENU_LOCK` (it only spawns work or shows windows).
fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id.as_ref();
    match id {
        "open" => crate::show_main(app),
        "quit" => app.exit(0),
        "dumps" => {
            let _ = crate::show_dumps(app);
        }
        "mail" => {
            let _ = crate::mail_window::show_mails(app);
        }
        "new-site" => {
            crate::show_main(app);
            let _ = app.emit("sites-intent", "create");
        }
        "update:apply" => {
            crate::show_main(app);
            let _ = app.emit("navigate", "/about");
        }
        "update:check" => spawn_update_check(app.clone()),
        "daemon:start" => spawn_lifecycle(app.clone(), Lifecycle::Start),
        "daemon:restart" => spawn_lifecycle(app.clone(), Lifecycle::Restart),
        "daemon:stop" => spawn_lifecycle(app.clone(), Lifecycle::Stop),
        _ => {
            if let Some(version) = id.strip_prefix("php:set:") {
                spawn_set_default_php(app.clone(), version.to_string());
            } else if let Some(route) = id.strip_prefix("nav:") {
                crate::show_main(app);
                let _ = app.emit("navigate", route.to_string());
            }
            // `noop:*` labels and any unknown id (incl. the window-control menu's
            // `close-window`/`minimize-window`) fall through here, ignored.
        }
    }
}

fn on_tray_icon_event(tray: &TrayIcon, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        crate::show_main(tray.app_handle());
    }
}

#[derive(Clone, Copy)]
enum Lifecycle {
    Start,
    Restart,
    Stop,
}

/// Clears `TRANSITION` on drop, so even a panicking lifecycle task or a runtime
/// shutdown mid-action can't leave the tray and poller frozen on "Restarting…".
struct TransitionGuard;
impl Drop for TransitionGuard {
    fn drop(&mut self) {
        TRANSITION.store(false, Ordering::Release);
    }
}

/// Run a daemon lifecycle action while owning the menu: claim `TRANSITION` (a
/// second click while one is in flight is ignored - the transient menu also hides
/// the lifecycle items), show a transient menu, await the bounded settle, then
/// rebuild from fresh state. The final apply clears `TRANSITION` under the lock;
/// the `TransitionGuard` is a backstop that clears it on any abnormal exit.
fn spawn_lifecycle(app: AppHandle, action: Lifecycle) {
    if TRANSITION
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return; // another lifecycle action already owns the menu
    }
    tauri::async_runtime::spawn(async move {
        let _guard = TransitionGuard;
        let label = match action {
            Lifecycle::Start => "Starting…",
            Lifecycle::Restart => "Restarting…",
            Lifecycle::Stop => "Stopping…",
        };
        apply_transient(&app, label);

        match action {
            Lifecycle::Start => {
                let _ = crate::daemon::start(app.clone(), true).await;
                wait_until_reachable(true).await;
            }
            Lifecycle::Restart => {
                let prev = current_boot_id().await;
                // `RestartDaemon` replies Ok *before* the re-exec (then the socket
                // briefly drops); we act only on the boot_id change, but bound the
                // call so a wedged daemon can't hang the task with TRANSITION set.
                let _ = exchange_timeout(&Request::RestartDaemon, PROBE_TIMEOUT).await;
                wait_until_restarted(prev).await;
            }
            Lifecycle::Stop => {
                let _ = crate::daemon::stop().await;
                wait_until_reachable(false).await;
            }
        }

        let state = fetch_state().await;
        let guard = lock_menu();
        apply(&app, &state);
        TRANSITION.store(false, Ordering::Release);
        drop(guard);
    });
}

fn spawn_set_default_php(app: AppHandle, version: String) {
    let Ok(version) = PhpVersion::from_str(&version) else {
        return; // not a parseable version id; ignore
    };
    tauri::async_runtime::spawn(async move {
        let _ = exchange(&Request::SetDefaultPhp { version }).await;
        refresh_now(&app).await;
    });
}

fn spawn_update_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Live check (network) bounded by a timeout; `None` channel resolves the
        // daemon's persisted preference.
        let _ = exchange_timeout(
            &Request::CheckUpdate { channel: None },
            Duration::from_secs(20),
        )
        .await;
        refresh_now(&app).await;
    });
}

/// Read the daemon's current `boot_id` (the restart-completion key).
async fn current_boot_id() -> Option<u64> {
    match exchange_timeout(&Request::Status, PROBE_TIMEOUT).await {
        Ok(Response::Status { report }) => report.boot_id,
        _ => None,
    }
}

/// Bounded-poll until the daemon's reachability matches `want`.
async fn wait_until_reachable(want: bool) {
    for _ in 0..SETTLE_STEPS {
        tokio::time::sleep(SETTLE_STEP).await;
        let reachable = matches!(
            exchange_timeout(&Request::Status, PROBE_TIMEOUT).await,
            Ok(Response::Status { .. })
        );
        if reachable == want {
            return;
        }
    }
}

/// Bounded-poll until a restart completes: with a known previous `boot_id`, until
/// the daemon is reachable with a *different* `boot_id` (the old process is
/// briefly alive with the old id); against an older daemon that sends no
/// `boot_id`, until an unreachable→reachable transition is observed.
async fn wait_until_restarted(prev: Option<u64>) {
    let mut saw_down = false;
    for _ in 0..SETTLE_STEPS {
        tokio::time::sleep(SETTLE_STEP).await;
        match exchange_timeout(&Request::Status, PROBE_TIMEOUT).await {
            Ok(Response::Status { report }) => match prev {
                Some(old) => {
                    if report.boot_id.is_some_and(|new| new != old) {
                        return;
                    }
                }
                None => {
                    if saw_down {
                        return;
                    }
                }
            },
            _ => saw_down = true,
        }
    }
}
