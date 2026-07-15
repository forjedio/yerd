# MCP

`yerd mcp` serves Yerd's tools to AI agents over the [Model Context
Protocol](https://modelcontextprotocol.io). The [AI Agents
guide](../../guide/ai-agents) covers enabling it and registering it with your
agent; this page is the command reference.

::: info You don't run this by hand
An agent spawns `yerd mcp` and talks to it over the pipe. Run it in a terminal
and it just sits there waiting for JSON-RPC on stdin - useful only for debugging.
:::

## Commands

| Command | Description |
| --- | --- |
| `yerd mcp` | Serve one MCP session on stdin/stdout until the client disconnects. |

Register it once, rather than running it:

```sh
claude mcp add --scope user yerd -- yerd mcp
```

## `yerd mcp`

Takes no arguments.

Speaks MCP over **stdio**: newline-delimited JSON-RPC 2.0 in on stdin, one
message per line out on stdout. Each tool call is forwarded to the
[`yerdd` daemon](../../guide/daemon) over the same IPC socket the rest of the CLI
uses, so an agent sees exactly the state `yerd` and the app see.

- **stdout carries the protocol only.** Every log line and diagnostic goes to
  stderr, so it never corrupts the stream.
- **Exit code is `0`** on a clean disconnect (EOF on stdin), which is the normal
  way a session ends when the agent shuts down.
- **The daemon does not have to be running** when the session starts. Tool calls
  made while it is down come back as failed tool results explaining that Yerd
  needs starting, rather than killing the session.

### Gating

Tools are served only when **AI Agents** is enabled in Yerd's General settings
(the `mcp_enabled` config key). The gate is read from the daemon, not from the
config file directly:

- **Enabled** - tools run.
- **Disabled** - the session still connects and lists its tools, and each call
  returns a failed result telling the agent to ask you to enable it. The
  handshake deliberately succeeds: a server that failed to start would look
  broken rather than switched off.
- **Unknown** (the daemon was unreachable at startup) - the session reports that
  it could not check, rather than claiming the feature is off.

While a session is not enabled it re-checks the setting on each tool call, so
turning the toggle on reaches a running agent immediately. Once a session is
serving, it stops checking - turning the toggle off does not interrupt work in
progress, and applies to sessions started afterwards.

### Protocol support

| Item | Value |
| --- | --- |
| Transport | stdio (newline-delimited JSON-RPC 2.0) |
| Protocol revisions | `2025-11-25` (offered), `2025-06-18`, `2025-03-26`, `2024-11-05` |
| Capabilities | `tools` |
| Methods | `initialize`, `ping`, `tools/list`, `tools/call` |

The client's requested revision is echoed back when Yerd supports it; otherwise
Yerd offers its newest and the client decides whether to proceed. Batched
(JSON array) requests are rejected - batching was removed from MCP in
`2025-06-18`, and no stdio client sends them.

## See also

- [AI Agents](../../guide/ai-agents) - enabling the server, the tool list, and troubleshooting.
- [Daemon control](./daemon) - the daemon each tool call is forwarded to.
