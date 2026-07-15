# AI Agents

Yerd ships an [MCP](https://modelcontextprotocol.io) server, so AI coding agents
— Claude Code, Cursor, VS Code Copilot, and anything else that speaks the
protocol — can drive your local environment directly: list and create sites, set
PHP versions, wire up proxies, create databases, and read the mail and dumps your
apps produce.

It is **off by default**. Nothing is exposed until you turn it on.

## Enable it

Open Yerd → **Settings** → **General** → **AI Agents (MCP)** and turn on
**Enable Yerd's MCP server**.

The switch writes `mcp_enabled` to your config; Yerd's daemon does not run a
server for it. Each agent session runs a short-lived `yerd mcp` process that
reads the setting and serves tools over its own stdin/stdout.

That means:

- **Turning it on** reaches agent sessions that are already running — their next
  tool call goes through. No restart needed.
- **Turning it off** applies to sessions started afterwards. A session already
  using Yerd keeps working until it restarts.

::: warning Not a security boundary
The toggle is a convenience control, not a sandbox. Any program running as your
user can already talk to Yerd's daemon through its socket — that is how the
`yerd` CLI works. Turning this off stops agents from *discovering* Yerd's tools;
it does not isolate Yerd from local software.
:::

## Register Yerd with your agent

You only do this once. The `yerd` binary must be on your `PATH` — on macOS use
Settings → General → **Terminal CLI** → Install to put it there.

For Claude Code:

```sh
claude mcp add --scope user yerd -- yerd mcp
```

For agents that use a JSON config file (Cursor, VS Code, and most others):

```json
{
  "mcpServers": {
    "yerd": {
      "command": "yerd",
      "args": ["mcp"]
    }
  }
}
```

The GUI card shows both, with a copy button, once the server is enabled.

To check it worked in Claude Code, run `/mcp` — `yerd` should be listed as
connected, with its tools available.

## What agents can do

| Area | Tools |
| --- | --- |
| Sites | `list_sites`, `create_site`, `link_site`, `park_directory`, `list_parked`, `set_site_php`, `set_site_secure` |
| Domains | `add_domain`, `remove_domain` |
| Proxies | `add_proxy`, `remove_proxy`, `add_proxy_rule`, `remove_proxy_rule`, `list_proxies` (upstreams must be local) |
| PHP | `list_php`, `list_available_php`, `install_php`, `set_default_php`, `set_php_setting` |
| Services | `list_services`, `list_databases`, `create_database` |
| Mail | `set_mail_enabled`, `list_mails`, `get_mail` |
| Dumps | `set_dumps_enabled`, `dumps_status`, `list_dumps` |
| Health | `status`, `diagnose`, `job_status` |

### What agents cannot do

There are **no destructive tools**. An agent can create and configure, but it
cannot drop a database, uninstall PHP or a service, unlink or unpark a site,
clear your captured mail or dumps, or share a site publicly through a tunnel.

**Nothing an agent can do sends your data off the machine.** Proxy upstreams are
restricted to this machine (`localhost`, `127.0.0.0/8`, `::1`), even though
`yerd proxy add` itself accepts any host. A proxy rule pointing a `.test` site at
a remote server would make your browser hand that server the site's cookies and
tokens, over an origin your system already trusts — so an agent may not create
one, for the same reason it may not open a tunnel.

Both limits exist because an agent acts on what it reads, and what it reads can
be wrong or hostile. Use `yerd` or the app for anything on these lists — as a
human, deliberately.

## Long-running work

`create_site` and `install_php` take far longer than a single call, so they start
a background job and return a `job_id` straight away. The agent then polls
`job_status` with that id — passing the `next_cursor` from each poll to get only
new log lines — until `state` reads `succeeded`, `failed`, or `cancelled`. Yerd
tells the agent this in the tool's own description, so it handles the flow
itself.

Jobs are held in memory. If the daemon restarts mid-job, the id is gone: the
agent is told as much and asked to verify the outcome directly (with
`list_sites` or `list_php`) rather than assuming it failed.

## Troubleshooting

**The server shows as connected but every tool says it is disabled.** The toggle
is off. Turn it on in Settings → General; the running session picks it up on its
next call.

**The server fails to start / shows red.** The agent could not run `yerd mcp` —
almost always `yerd` not being on the `PATH` the agent inherits. Install the
Terminal CLI (Settings → General), then restart the agent so it picks up the new
`PATH`.

**Tools report the daemon is not running.** Yerd itself is stopped. Open the Yerd
app (or run `yerdd`) and retry.

## See also

- [`yerd mcp`](../reference/cli/mcp) - the command reference and its stdio contract.
- [Mail Capture](./mail) and [Laravel Dumps](./laravel-dumps) - the features
  behind the mail and dump tools.
- [The Daemon](./daemon) - what agents are actually talking to.
