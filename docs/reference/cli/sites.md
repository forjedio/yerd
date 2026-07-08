# Sites

A directory tree is served in one of two ways: **park** a parent directory so each child folder automatically becomes a `<child>.test` site, or **link** a single directory under an explicit name. See the [Sites guide](../../guide/sites) for the full model.

| Command | Description | Example |
| --- | --- | --- |
| `yerd sites` | List every parked or linked site. | `yerd sites` |
| `yerd park <PATH>` | Park a directory: each of its child directories becomes a `.test` site. | `yerd park ~/Sites` |
| `yerd unpark <PATH>` | Un-park a directory so its children stop being served. Linked sites are untouched. | `yerd unpark ~/Sites` |
| `yerd link` | Link the current directory, named after its folder. | `yerd link` |
| `yerd link <NAME>` | Link the current directory under an explicit name. | `yerd link blog` |
| `yerd link <PATH>` | Link a directory, named after its folder. | `yerd link ~/code/blog` |
| `yerd link <NAME> <PATH>` | Link a directory under an explicit name. | `yerd link blog ~/code/blog` |
| `yerd unlink <NAME>` | Remove a linked site by name. | `yerd unlink blog` |
| `yerd root <NAME> <PATH>` | Set the served directory (web root) for a site, relative to its folder. | `yerd root blog public` |
| `yerd root <NAME> --auto` | Reset a site to automatic web-root detection. | `yerd root blog --auto` |

```sh
# Park a folder of projects: every subdirectory is reachable at <name>.test
yerd park ~/Sites

# Link one project under a specific name (serves https://blog.test once secured)
yerd link blog ~/code/blog

# Link the current project, named after its folder
cd ~/code/blog && yerd link

# See everything yerd is serving
yerd sites
```

`yerd sites` prints a table with the columns `NAME`, `KIND` (`parked` or `linked`), `PHP`, `SECURE`, `SERVED`, and `DOCROOT`. `SERVED` is the web root relative to the document root (`/` means the project root itself is served). When there are no sites it prints `no sites`. A `DOMAIN` column appears only when at least one site has a customised primary domain or a shadowed apex; each cell holds the site's primary FQDN, or `apex shadowed by <site>` when another site claims its apex, or `-`. Use [`yerd domain list`](./domains) to see the full per-site domain set (including subdomains and wildcards).

::: details How site names are validated
`link`, `unlink`, `secure`, `unsecure`, and `root` validate the name client-side before connecting: a name must be a single valid DNS label. A bad name (e.g. `bad name` or `bad/name`) fails immediately with a usage error and exit code `2`, before any request reaches the daemon.

`link` accepts a single positional argument as either a name or a path: an argument containing a path separator (or `.`/`..`) is treated as a directory, and the site name is derived from its folder name (lowercased, with runs of invalid characters collapsed to a single `-`); a bare word is always treated as a name, even if a same-named subdirectory happens to exist. With no arguments at all, the current directory is linked and named after its own folder.
:::

::: tip Web root detection
Yerd auto-detects the directory each site is served from (e.g. `public/` for Laravel, the project root for WordPress). For a **parked** site it re-detects continuously as the project changes. For a **linked** site detection runs once, when the site is first linked; it isn't re-run automatically afterward. `yerd root <name> <path>` pins it explicitly for either kind. `yerd root <name> --auto` (or with no path) returns to auto-detection: for a linked site this re-runs the one-shot detection immediately and pins the fresh result; for a parked site it clears the pin and hands the site back to the continuous watched detection. The path must resolve to a directory inside the site's folder. See the [Sites guide](../../guide/sites#web-root-the-served-directory).
:::

::: warning About `unpark`
The daemon stores parked roots by their canonical path and matches `unpark` against that stored string **exactly**. `yerd` best-effort canonicalises the path you type (resolving symlinks and relative paths) so it matches. If the directory has been deleted from disk it can't be canonicalised, so pass the exact stored path instead. Run `yerd list parked` to see the canonical paths the daemon holds, including empty roots that produce no sites and therefore don't show up in `yerd sites`.
:::
