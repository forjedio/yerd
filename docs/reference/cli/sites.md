# Sites

A directory tree is served in one of two ways: **park** a parent directory so each child folder automatically becomes a `<child>.test` site, or **link** a single directory under an explicit name. See the [Sites guide](../../guide/sites) for the full model.

| Command | Description | Example |
| --- | --- | --- |
| `yerd sites` | List every parked or linked site. | `yerd sites` |
| `yerd park <PATH>` | Park a directory: each of its child directories becomes a `.test` site. | `yerd park ~/Sites` |
| `yerd unpark <PATH>` | Un-park a directory so its children stop being served. Linked sites are untouched. | `yerd unpark ~/Sites` |
| `yerd link <NAME> <PATH>` | Link a single directory as a named site. | `yerd link blog ~/code/blog` |
| `yerd unlink <NAME>` | Remove a linked site by name. | `yerd unlink blog` |
| `yerd root <NAME> <PATH>` | Set the served directory (web root) for a site, relative to its folder. | `yerd root blog public` |
| `yerd root <NAME> --auto` | Reset a site to automatic web-root detection. | `yerd root blog --auto` |

```sh
# Park a folder of projects: every subdirectory is reachable at <name>.test
yerd park ~/Sites

# Link one project under a specific name (serves https://blog.test once secured)
yerd link blog ~/code/blog

# See everything yerd is serving
yerd sites
```

`yerd sites` prints a table with the columns `NAME`, `KIND` (`parked` or `linked`), `PHP`, `SECURE`, `SERVED`, and `DOCROOT`. `SERVED` is the web root relative to the document root (`/` means the project root itself is served). When there are no sites it prints `no sites`.

::: details How site names are validated
`link`, `unlink`, `secure`, `unsecure`, and `root` validate the name client-side before connecting: a name must be a single valid DNS label. A bad name (e.g. `bad name` or `bad/name`) fails immediately with a usage error and exit code `2`, before any request reaches the daemon.
:::

::: tip Web root detection
Yerd auto-detects the directory each site is served from (e.g. `public/` for Laravel, the project root for WordPress) and re-detects when the project changes. `yerd root <name> <path>` pins it explicitly; `yerd root <name> --auto` (or with no path) returns to auto-detection. The path must resolve to a directory inside the site's folder. See the [Sites guide](../../guide/sites#web-root-the-served-directory).
:::

::: warning About `unpark`
The daemon stores parked roots by their canonical path and matches `unpark` against that stored string **exactly**. `yerd` best-effort canonicalises the path you type (resolving symlinks and relative paths) so it matches. If the directory has been deleted from disk it can't be canonicalised, so pass the exact stored path instead. Run `yerd list parked` to see the canonical paths the daemon holds, including empty roots that produce no sites and therefore don't show up in `yerd sites`.
:::
