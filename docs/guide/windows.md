# Windows (early access)

Yerd runs on Windows 10/11 (x86-64) as an **early-access** build. It installs and
runs entirely **per-user, with no administrator prompt at install time**, serves
`.test` sites over HTTP and HTTPS with a per-user trusted CA, dispatches a PHP
version per site, and self-updates through the installer. This page covers the
install flow and the known MVP limitations.

## Install

1. Download `Yerd_Windows_x86_64_v<ver>.exe` from the
   [releases page](https://github.com/forjedio/yerd/releases).
2. Run it. The installer is currently **unsigned**, so Windows SmartScreen shows
   *"Windows protected your PC"* on first run - click **More info → Run anyway**.
   (This appears only for the first manual download; in-app self-updates do not
   hit SmartScreen, because the update is verified by SHA-256 + minisign, not by
   Authenticode.)
3. It installs per-user into `%LOCALAPPDATA%\Yerd` - **no UAC prompt** at install.
4. On first launch the app starts its bundled daemon and walks you through a
   one-time trust step (this raises **one** UAC prompt to add the `.test` DNS
   rule; the CA is added to your user certificate store with a per-cert
   confirmation dialog).
5. Optionally open *Settings → Terminal CLI → Install* to add `yerd` and the tool
   shims (`php`, `composer`, ...) to your user `PATH`. Open a new terminal
   afterward.

Autostart is a **per-user logon entry** (an `HKCU\...\Run` value), enabled from
the app - not a Windows Service. The installer registers no service and no
autostart entry; the app owns that decision.

## Certificate trust and browsers

The local CA goes into your **CurrentUser** Root store (the one-time confirmation
dialog in step 4). That store is per-user by design: it is what lets Yerd trust
the CA with no administrator prompt. Every browser reads it, so there is no
`certutil`, no NSS tooling, and no manual import on Windows.

- **Edge, Chrome and other Chromium-family browsers** use the Windows store
  directly and pick the CA up immediately.
- **Firefox** imports the Windows user Root store too, through its
  `security.enterprise_roots.enabled` preference, which is **on by default**.

**Restart Firefox after `yerd elevate trust`.** Firefox reads the Windows root
store once, at startup, so a CA added (or removed) while it is running is not
picked up until you restart it. Edge and Chrome see the change straight away.

::: tip Firefox alone still warns?
Restart it first. If it still warns, check `security.enterprise_roots.enabled` in
`about:config` - it ships `true`, but a hardening `user.js` or an earlier manual
change can turn it off.

Check trust from the **padlock**, not from `about:certificate`: on a secured
`.test` site, click the padlock and open the connection details, where a trusted
site reads *Verified by: Yerd Local CA*. The CA deliberately does **not** appear
in `about:certificate` even when Firefox trusts it - that tab lists Firefox's own
certificate database, and roots imported from the operating system are held
separately from it. Looking there and seeing nothing is not a fault.
:::

## Known MVP limitations

These are tracked and slated for hardening after early access:

- **Unsigned installer.** Expect the SmartScreen "Run anyway" step above until a
  signing certificate is wired in (a dormant seam already exists in the bundle
  config). Some corporate AppLocker/WDAC policies may block an unsigned installer
  outright.
- **No system metrics in the GUI.** CPU/RAM tiles that appear on macOS/Linux are
  not yet wired on Windows.
- **Brief console flash at logon.** The daemon's autostart entry can flash a
  short-lived console window at logon (cosmetic).
- **ACL hardening is best-effort.** The runtime directory and the CA private key
  get an `icacls` DACL granting only your own account (the daemon's named pipe
  carries its own security descriptor), but if `icacls` cannot be run Yerd logs a
  warning and carries on with the inherited default ACL rather than refusing to
  start.
- **Four concurrent PHP requests per version.** Windows uses `php-cgi`, which
  serves one request at a time, so Yerd runs **four** of them per PHP version on
  separate loopback ports and rotates requests across them. They start on
  demand, so a version that is only ever asked for one request at a time never
  runs more than one process. Four is fixed in the build; it is not a setting.
  Two consequences worth knowing:
  - The **FPM pool size** setting is hidden on Windows, and `yerd php pool set`
    is refused there - php-cgi has no worker pool of its own to size.
  - A request that makes a **loopback HTTP call back into a site on the same PHP
    version** borrows a second worker, so it works, but a chain deeper than four
    (or three busy tabs plus a loopback) still blocks until it times out.
    WordPress's `wp-cron` loopback and an app calling its own API are the usual
    ways to hit this. Workarounds: put the caller and callee on different PHP
    versions, or disable the loopback (for WordPress, set
    `DISABLE_WP_CRON` and run `wp cron event run` from a terminal instead).
- **"Open in editor" and "Open folder" are not wired.** The IDE launcher and
  system-opener adapters still resolve to the unsupported stub on Windows, so
  those buttons return an error rather than opening anything.
- **Self-update replaces the installed app** via the silent installer
  (`/S /UPDATE`): the running executables are renamed aside, the installer runs,
  and the app + daemon restart on the new version. Your data, CA, DNS rule, and
  autostart entry are preserved across updates - only a real uninstall removes
  them.

## Uninstall

Uninstall from **Settings → Apps** (Add/Remove Programs). The uninstaller removes
the binaries, the autostart entries, the PATH entries, the local CA, the `.test`
DNS rule, and the data directories. A running `yerd.exe` copy on your PATH may
need a manual delete (a program can't delete its own image while running) - the
uninstaller notes anything it left behind.
