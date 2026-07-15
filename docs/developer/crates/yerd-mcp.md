# yerd-mcp

`yerd-mcp` is the **Model Context Protocol server logic** behind `yerd mcp`: the
tool catalog agents call, the JSON-RPC state machine that drives a session, and
the rendering of daemon answers back into tool results. It lets Claude Code,
Cursor, and other local agents manage Yerd directly.

The crate is **sans-io**. It never touches stdin, a socket, the clock, or the
environment; it turns one input line into a description of what the caller should
do next. The stdio loop and the daemon exchange live at the binary edge, in
[`yerd`](../binaries/yerd)'s `mcp_cmd` module.

::: info Crate metadata
`description`: *Model Context Protocol server logic: tool catalog, JSON-RPC state
machine, and result rendering.* `#![forbid(unsafe_code)]`. Depends on
[`yerd-core`](./yerd-core) and [`yerd-ipc`](./yerd-ipc) (default features - the
pure build) plus `serde` / `serde_json` / `thiserror`. **No async runtime**, and
`tests/no_runtime_deps.rs` keeps it that way.
:::

See also the [Crates overview](../crates), [`yerd-ipc`](./yerd-ipc) (the requests
each tool maps to), the [`yerd mcp` command reference](../../reference/cli/mcp),
and the user-facing [AI Agents guide](../../guide/ai-agents).

## Module map

```text
src/
├── lib.rs        # Server / Outgoing / PendingCall / Availability + the layering doc
├── protocol.rs   # JSON-RPC envelope, method dispatch, the handshake
├── tools.rs      # the catalog: ToolDef table, schemas, build() -> yerd_ipc::Request
├── render.rs     # daemon Response -> MCP tool result (content / isError)
└── error.rs      # ArgError - argument validation, surfaced as JSON-RPC -32602
```

## The state machine

One tool = exactly one [`yerd_ipc::Request`](./yerd-ipc), so a call needs at most
one daemon round trip and the machine never has to track multi-step work.

```rust
let mut server = Server::new(Availability::Enabled, "2.0.3");
match server.handle_line(line) {
    Outgoing::Reply(line) => write(line),          // answer it yourself
    Outgoing::None => {}                           // a notification: never answer
    Outgoing::CallDaemon(call) => {                // one exchange, then complete
        let result = exchange(call.request.clone()).await;
        write(call.complete(result));
    }
    Outgoing::PolicyBlocked(call) => { /* re-check the gate, then run or refuse */ }
}
```

`PendingCall::complete` takes a `Result<Response, String>`. The `Err` side is not
an error type but the human-readable text describing a transport failure - the
caller has already decided what to tell the agent, and this renders it as a
failed tool result.

## Rules worth knowing before you edit

**Notifications are never answered.** `handle_line` branches on the presence of
`id` *before* method dispatch. A message with no `id` is a notification -
`notifications/cancelled` and friends are consumed silently; replying to one is a
protocol violation. A message with no `method` is a stray response and is
likewise ignored.

**`-32xxx` is for protocol faults, not policy.** A disabled toggle produces a
failed *tool result*, not a JSON-RPC error, and `initialize` always succeeds. A
server that failed its handshake because a feature was switched off would be
indistinguishable, in an agent's server list, from one that is broken.

**Validation beats the gate.** Tool lookup and argument checks run before the
availability check, so a malformed call is reported as malformed even while
gated - otherwise the agent is sent to fix the wrong thing.

**Guidance is rendered on demand.** `Server::gate_reply` reflects the *current*
availability. A session that starts with no daemon and later finds the toggle off
must stop blaming the daemon.

**Wire casing is exact.** `protocolVersion`, `serverInfo`, `inputSchema`, and
`isError` are camelCase per the spec; a snake_case `inputSchema` is silently
ignored by clients, which then see a schema-less tool. The golden test pins it.

## Tests / invariants

- `tests/protocol.rs` - the handshake matrix, the notification/request split,
  error-code mapping, the gate, and single-line framing.
- `tests/tools.rs` - the catalog name list is **pinned**; every entry is
  well-formed, builds its request, and exposes no destructive tool.
- `tests/render.rs` - content shape, error mapping, status trimming, job hints.
- `tests/no_runtime_deps.rs` - the manifest declares no async runtime. Note this
  checks the *manifest*, not the resolved graph like the other pure crates'
  guards: `cargo metadata` unifies features workspace-wide, and because `yerdd`
  enables `yerd-ipc/transport`, a reachability walk sees a `yerd-ipc -> tokio`
  edge no matter who asks.

Treat a catalog-pin failure as a contract alarm: the tool list is what agents
build their calls against.

## Design notes

**Why hand-rolled?** The needed surface is four methods and a tool table. Yerd
already hand-rolls DNS ([`yerd-dns`](./yerd-dns)) and SMTP
([`yerd-mail`](./yerd-mail)) for the same reason: the official SDK is
tokio-based and its async model would not fit a pure crate, while its dependency
tree would have to be pinned and audited against the workspace's 1.77 MSRV.

**Why curated tools, not the whole IPC surface?** Roughly 100 auto-generated
tools measurably degrade an agent's tool selection and cost tokens on every turn.
The catalog is also where destructive operations are kept out: v1 exposes nothing
that drops a database, uninstalls software, unlinks a site, or clears captured
data.
