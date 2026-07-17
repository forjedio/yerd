---
applyTo: "crates/yerd-mcp/**/*.rs"
---

# yerd-mcp — the MCP server logic

The tool catalog agents call, the JSON-RPC state machine that drives a session,
and the rendering of daemon answers into tool results.

**Layer:** pure and **sans-io**. No stdin/stdout, no socket, no clock, no env,
no async runtime. `handle_line(&str) -> Outgoing` describes what the caller
should do; the caller does it. The stdio loop and the daemon exchange live in
`bin/yerd`'s `mcp_cmd`.

## Owns

- `Server` / `Outgoing` / `PendingCall` / `Availability` — the session machine.
- `protocol.rs` — the JSON-RPC envelope, the handshake, method dispatch.
- `tools.rs` — the catalog: one entry per tool, each mapping to exactly one
  `yerd_ipc::Request`.
- `render.rs` — `Response` → tool result, including the `status` trim and the
  job-polling hint.

## Must not

- Depend on `yerd-ipc`'s `transport` feature, or on tokio, directly or
  transitively. `tests/no_runtime_deps.rs` guards the manifest.
- Reimplement daemon logic. A tool that needs a new capability needs a new
  `Request` in `yerd-ipc` and a daemon handler — not logic here.
- Expose a destructive tool (drop/uninstall/unlink/unpark/clear/delete/stop).
  That exclusion is the feature's safety story, and `tests/tools.rs` pins it.
- Add an **egress** path — anything that can send a developer's data off the
  machine. Tunnels are excluded for this reason, and proxy upstreams are
  loopback-only (`req_local_url`) even though the CLI allows any host. An agent
  can be talked into things a user cannot; a proxy rule aimed at a remote host
  hands over a trusted origin's cookies with no TLS warning.

## Protocol rules (easy to get wrong)

- **Never answer a notification.** Branch on the presence of `id` *before*
  dispatching a method. No `id` → consume and return `Outgoing::None`, whatever
  the method. No `method` → a stray response → also ignore.
- **`-32xxx` is for protocol faults, not policy.** A disabled toggle or an
  unreachable daemon is a failed *tool result* (`isError: true`). `initialize`
  always succeeds — a server that fails its handshake looks broken rather than
  switched off.
- **`initialize` and `ping` answer at any time**; only `tools/*` require that
  `initialize` has been answered. "Initialized" means *the server answered
  `initialize`*, not that the client sent `notifications/initialized`.
- **Validate before gating.** Unknown tool / bad args → `-32602` even while
  gated, or the agent is sent to fix the wrong thing.
- **Wire casing is exact**: `protocolVersion`, `serverInfo`, `inputSchema`,
  `isError`. A snake_case `inputSchema` is silently ignored by clients.
- **Replies are single-line.** stdout is newline-framed; an embedded newline
  desynchronises the client.

## Tests / invariants

- `tests/tools.rs` — the catalog name list is **pinned**, every entry is
  well-formed and builds its request, and no destructive tool appears. Treat a
  pin failure as a contract alarm: agents build their calls against this list.
- `tests/protocol.rs` — handshake matrix, notification/request split, error
  codes, the gate, single-line framing.
- `tests/render.rs` — content shape, error mapping, status trim, job hints.
- `tests/no_runtime_deps.rs` — the manifest declares no runtime.

## Review checklist

- [ ] Still pure: no I/O, no clock, no env, no tokio; MSRV 1.77 (no async
      closures — use a generic future bound).
- [ ] New/changed tool: catalog pin, schema, builder arm, and render path all
      updated together; description reads as one sentence.
- [ ] Nothing destructive added.
- [ ] Notifications still unanswered; policy still not reported as `-32xxx`.
- [ ] Typed (`thiserror`) errors; no `unwrap`/`expect`/panic/indexing.
