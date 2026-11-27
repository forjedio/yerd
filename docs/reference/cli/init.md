# Init

`yerd init` applies the current directory's [`.yerd.toml`](../project-file),
or creates one with a short wizard and then applies it. The
[Project File guide](../../guide/project-file) covers the workflow; this page is
the command reference.

## Commands

| Command | Description | Example |
| --- | --- | --- |
| `yerd init` | Apply `./.yerd.toml`, or run the wizard to create it when there is none. | `cd ~/code/shop && yerd init` |

```sh
# A teammate's clone: the file is committed, so this just applies it
git clone git@github.com:acme/shop.git && cd shop
yerd init

# Run it again at any time; anything already in place is "unchanged"
yerd init

# In CI or a script: apply and get one JSON object back
yerd --json init
```

## `yerd init`

Takes no arguments; it always works on the current directory. The daemon reads,
writes and applies the file, so the result is the same as applying it from the
desktop app.

### With a `.yerd.toml`

Prints `Applying <path>`, then one line per item as the daemon applies it:

| Prefix | Meaning |
| --- | --- |
| `applied:` | The item was changed to match the file (including `linked <name>`). |
| `installed:` | A missing PHP version, Node release or service engine was installed. |
| `started:` | An installed service was started. |
| `unchanged:` | The item already matched. |
| `warning:` | The item was left alone, for example a domain another site serves (`...; skipped`), a service installed at another version, or a missing extension. The command still succeeds. |
| `failed:` | The item couldn't be applied, for example a download that failed. Other items still run. |

The last line is a summary such as
`done: 3 applied, 5 unchanged, 1 warning(s), 0 failed`. Any `failed:` item ends
the run with `yerd: N item(s) could not be applied` and exit code `1`. Many
lines carry the command that fixes them, such as
`yerd install php 8.1 --legacy`, `yerd service change-version mysql <version>`,
`yerd xdebug on 8.4` or `yerd php ext add 8.4 <path-to.so>`.

A `.yerd.toml` that is invalid, a symlink or over 64 KiB is refused before
anything is applied, with the reason (exit code `1`).

### Without a `.yerd.toml`

Runs a wizard in the terminal. Each question shows its default in brackets;
press Enter to keep it. Defaults come from the site already serving the folder,
or from the folder name and your default PHP version for a new one.

| Question | Accepts |
| --- | --- |
| `Site name` | A site name; it is normalised like a parked folder's name. |
| `PHP version` | `major.minor`, such as `8.4`. |
| `Serve over HTTPS?` | `y` or `n`. |
| ``Node version (blank = not managed, `none` clears)`` | A Node selector such as `22` or `lts/*`. Blank keeps the default; `none` leaves Node unmanaged. |
| `Services (comma list of engine[@version], e.g. mysql@8.4, redis)` | Engines with an optional version prefix; no version means any (`*`). |
| `Extra domains without the TLD (comma list, e.g. admin.acme, *.acme)` | Domains, subdomains and wildcards without `.test`; an answer that ends in your TLD is asked again. |
| `Required PHP extensions (comma list, e.g. redis, imagick)` | Module names. |

For every list, `none` clears the prefilled value. An invalid answer prints the
reason and asks again. When the folder has a `.nvmrc` or `.node-version`, a note
before the Node question says it wins over a version set here. The site's
existing response headers and path proxy rules are included without a
question.

The wizard then prints the file and asks `Write .yerd.toml and apply it? [Y/n]`.
Answering yes writes the file and applies it as above. Answering no prints
`Nothing written.` and exits `1`.

The wizard needs an interactive terminal. With stdin not a terminal, or with
`--json`, `yerd init` in a folder without a `.yerd.toml` prints
``yerd: no .yerd.toml here - run `yerd init` in a terminal to create one`` to
stderr and exits `2` (with `--json`, the failure object below is printed to
stdout too). Ending input early (Ctrl-D) aborts without writing
anything.

### JSON output

`yerd --json init` (only with an existing file) prints nothing while it runs,
then one object when the job ends:

```json
{
  "state": "succeeded",
  "log": [
    "unchanged: site shop",
    "unchanged: PHP 8.4",
    "done: 0 applied, 2 unchanged, 0 warning(s), 0 failed"
  ],
  "error": null
}
```

`state` is `succeeded`, `failed` or `cancelled`; `error` is the job's error
message or `null`. Every other failure also prints exactly one object, with
`state` `failed`, the reason in `error` and in `log` any lines received
before the failure (empty when the apply never started), then exits with the
code below. That covers reading the file (an invalid `.yerd.toml`, or a folder
that doesn't exist), the daemon not running or the connection dropping, no
`.yerd.toml` in the folder, and the daemon refusing to start or follow the
apply. The same reason is also printed to stderr as `yerd: <reason>`.

```json
{ "state": "failed", "log": [], "error": "<reason>" }
```

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The file was applied (warnings included). |
| `1` | An item failed, the job was cancelled, the daemon refused the file (including an invalid `.yerd.toml`), or the wizard was declined or aborted. |
| `2` | No `.yerd.toml` and no terminal to run the wizard (or `--json`). |
| `69` | The daemon isn't running. |
| `74` | Another transport or I/O failure. |

## See also

- [Project File guide](../../guide/project-file) - workflow, write-back sync and limitations.
- [`.yerd.toml` reference](../project-file) - every key and rule.
- [Sites](./sites) - `yerd link` and friends.
