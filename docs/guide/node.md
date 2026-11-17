# Node.js

Yerd installs [Node.js](https://nodejs.org) (with `npm` and `npx`) as
self-contained releases in its own data directory, side by side, so different
projects can use different versions without `nvm`, Homebrew or a system install.
One release is the **global default**; a project can ask for another with an
`.nvmrc` or `.node-version` file, or a saved per-site preference.

## In the desktop app

Open the **Node** page from the sidebar (under **Environment**):

<ThemedImage light="/images/node-versions-light.png" dark="/images/node-versions-dark.png" alt="The Node page listing Node 26.13.1 (the default) and Node 24.22.3 with their LTS codenames" />

- **Installed versions** lists each release with its LTS codename and a star on
  the global default. Each row offers **Set default**, **Uninstall**, and
  **Update** when a newer patch of that major exists. The daemon checks for
  newer patches every 12 hours alongside the PHP update check (only while a
  Node release is installed; notify-only, nothing is downloaded but the
  release index). Pending updates show as a count on the sidebar's **Node**
  item, a red dot on the tray icon, and an **Update Node** tray item that
  opens this page. **Refresh** checks immediately.
- **Install** opens a dialog with a dropdown of the release lines you don't
  have yet, newest first, each showing its newest patch, LTS codename, and an
  **EOL** badge for end-of-life majors; the newest supported LTS is preselected.
  End-of-life majors are hidden unless you turn on **Show end-of-life
  versions**. A switch in the dialog lets you type an exact version instead. Majors below 9
  aren't offered. If the release schedule can't be fetched the list still
  loads, without EOL badges.

<ThemedImage light="/images/node-install-light.png" dark="/images/node-install-dark.png" alt="The Install a Node version dialog, with the exact-version and end-of-life switches and Node 22 preselected" />

- Installs run in the background with a live log you can close; the operations
  indicator keeps tracking them. Other version actions are disabled until the
  install finishes. The first install becomes the global default.
- **Update** installs the newest patch of that major alongside the old one. The
  global default moves to it only when the default currently resolves to an
  older patch of that major. Saved site preferences are never changed, and the
  old patch stays installed.
- **Uninstall** removes a single release. While other releases are installed, a
  release can't be removed if the global default would then resolve to nothing
  (for example an exact default naming it) - set another default first. Removing the last release is allowed and clears the default. Saved site
  preferences that resolve only to the removed release are cleared; the
  confirmation lists them first, and those sites fall back to their project file
  or the global default.

## Per-site versions

<ThemedImage light="/images/node-site-selectversion-light.png" dark="/images/node-site-selectversion-dark.png" alt="A site's details sidebar with the Node version row set to Default" />

Each site's details sidebar has a **Node** row: pick a saved preference, or
**Default** to clear it (the project file, then the global default, decide). A
warning shows when a project file takes precedence over the saved preference.
`node` and `npm` on your PATH still use the global default; run `yerd exec node …`
or `yerd exec npm …` in the project folder to use the site's version. Nothing installs
automatically: when the project file or saved preference names a version that
isn't installed, the row shows the error with an **Install Node X** button that
installs it right there (with live progress), and a link to the Node page.

## From the command line

Node releases can be installed side by side and selected from the CLI:

```sh
yerd install tool node 24
yerd install tool node lts/iron
yerd node use 24
yerd node use 22 --site blog
yerd node unset --site blog
yerd node uninstall 22.9.0
yerd exec npm install
```

`yerd exec node`, `npm`, and `npx` honor a project's `.nvmrc` or `.node-version`
(including `lts/*` and `lts/<codename>` such as `lts/iron`), then its saved
preference, then the global default. Bare `node`, `npm`, and `npx` on your `PATH`
always use the global default; run `yerd exec npm …` inside a project to use its
version. CLI installs stream their progress and keep an existing default.
`yerd uninstall tool node` removes every release and clears the default and all
site preferences. See [Node version commands](../reference/cli/tooling#node-versions)
and [resolution rules](../reference/cli/exec#node-resolution).

## Where things live

| Path | Contents |
|---|---|
| `{data}/tools/node-versions/<version>/` | One unpacked Node release (side by side). |
| `{data}/tools/node/node-<ver>-<os>-<arch>/` | A legacy single-version Node install, still usable. |
| `{data}/bin/{node,npm,npx}` | The `PATH` shims, pointing at the global default. |

Downloads are verified against the release's `SHASUMS256.txt` before they are
unpacked. `{data}` is Yerd's per-user data directory (`yerd status` shows it);
put `{data}/bin` on your `PATH` as described in
[Tooling](./tooling#put-yerd-s-bin-directory-on-your-path).

## See also

- [Node version commands](../reference/cli/tooling#node-versions) - the CLI surface.
- [Node resolution](../reference/cli/exec#node-resolution) - how `yerd exec` picks a version.
- [Configuration: Node preferences](../reference/configuration#node-preferences).
- [Tooling](./tooling) - Composer, Bun, pnpm, the Laravel installer and WP-CLI.
