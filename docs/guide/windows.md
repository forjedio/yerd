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

## Known MVP limitations

These are tracked and slated for hardening after early access:

- **Per-user cert trust only.** The local CA is added to your **CurrentUser**
  Root store (a one-time confirmation dialog). Edge and Chrome use that store and
  work out of the box. **Firefox needs manual trust** - it uses its own NSS
  store and Yerd does not yet auto-trust it (`certutil`/NSS integration is a
  post-MVP item).
- **Unsigned installer.** Expect the SmartScreen "Run anyway" step above until a
  signing certificate is wired in (a dormant seam already exists in the bundle
  config). Some corporate AppLocker/WDAC policies may block an unsigned installer
  outright.
- **No system metrics in the GUI.** CPU/RAM tiles that appear on macOS/Linux are
  not yet wired on Windows.
- **Brief console flash at logon.** The daemon's autostart entry can flash a
  short-lived console window at logon (cosmetic).
- **ACL hardening is a tracked TODO.** The CA key and runtime directory are not
  yet locked down with restrictive ACLs.
- **One concurrent PHP request per version.** Windows uses `php-cgi` (a worker
  pool is post-MVP), so heavy parallel requests to the same PHP version serialize.
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
