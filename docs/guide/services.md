# Services (Roadmap)

::: warning Not shipped yet
Database and cache services are planned, not available. There is no `yerd services` command, no `yerd install mysql`, and no way to start a database through Yerd today. This page describes the intended design, and everything is subject to change before it ships.
:::

Yerd's [roadmap](https://github.com/forjedio/yerd#roadmap) includes service supervision: managing MySQL, MariaDB, PostgreSQL, and Redis the way [DBngin](https://dbngin.com) does, as native child processes that Yerd starts, stops, and supervises. No Docker, no containers, no VM. Just prebuilt binaries downloaded on demand, checksum-verified, and run as your user under the [`yerdd` daemon](./daemon).

Need a database right now? Install one with your OS package manager (or DBngin) and point your app at it. Yerd won't interfere.

## Why native processes (not Docker)

Service support follows the same model as [PHP versions](./php-versions):

- Binaries are downloaded only when you ask for them, so Yerd stays small.
- They're SHA-verified, like Yerd's own release artifacts and prebuilt PHP builds.
- `yerdd` supervises one process per enabled service: start on boot, restart on crash, report health.
- Everything runs as your user on loopback, with no elevation. See the [privilege model](./elevation).

::: info DBngin-style, but integrated
Same one-click convenience, folded into the daemon you already run for sites, PHP, HTTPS, and DNS. A single `yerd status` shows the whole stack.
:::

## Planned services

The known service identifiers are already pinned in [`yerd-config`](../developer/crates/yerd-config):

| Service | Identifier | Kind |
|---|---|---|
| MySQL | `mysql` | Relational database |
| MariaDB | `mariadb` | Relational database |
| PostgreSQL | `postgres` | Relational database |
| Redis | `redis` | In-memory cache / store |

These four are the complete planned set for the first iteration.

## What already exists in the code

Supervision isn't implemented, but the config schema already reserves a services section so enabling a service later won't be a breaking change:

```toml
# ~/.config/yerd config (illustrative - see the Configuration Reference)
[services]
enabled = ["mysql"]
```

- It lives in [`yerd-config`](../developer/crates/yerd-config) as an `enabled` set of identifiers the daemon would auto-start.
- Identifiers are validated against the known list; unknown names are rejected.
- It's stored as plain strings on purpose. A future `yerd-services` crate will own the typed `Service` enum, and keeping config stringly-typed for now keeps the schema stable.

::: warning Don't hand-edit this yet
The `[services]` section parses, but nothing reads it to start a database. Adding entries does nothing today. Wait for the feature and drive it through the CLI or GUI.
:::

## Planned workflow (illustrative)

When it lands, it should feel like managing PHP versions. The commands below do not exist yet:

```sh
# Roadmap - illustrative only. None of these run today.
yerd install mysql        # download + SHA-verify a prebuilt MySQL build
yerd services             # list services and their status
yerd start mysql          # supervised native process, no Docker
yerd stop mysql
```

The PHP workflow it mirrors, which does work today:

```sh
yerd install php 8.5      # download a prebuilt static PHP build
yerd list php             # list installed versions
yerd restart php 8.5      # restart the supervised FPM pool
yerd status               # daemon, ports, DNS, CA, PHP pools (PID/RAM), load
```

## Windows and Redis licensing

[Windows support is itself on the roadmap](../developer/cross-platform), and Redis adds a wrinkle: there's no official native Windows server, and Redis licensing has shifted in recent years. Shipping a checksum-verified, redistributable Redis binary for Windows is a separate, unresolved question from doing so on macOS and Linux.

::: info High-level only
A known caveat, not a committed plan. How Redis is offered on Windows (official server, a fork, or an alternative) gets decided when Windows service support is designed. On macOS and Linux, native server builds are available, so Redis is in the planned first set.
:::

## Following along

- Watch the [roadmap in the README](https://github.com/forjedio/yerd#roadmap).
- Browse the [source on GitHub](https://github.com/forjedio/yerd).
- See [Contributing](../developer/contributing) if you'd like to help build it.

## See also

- [CLI Reference](../reference/cli/) - commands in the current release
- [PHP Versions](./php-versions) - the supervision model services will follow
- [Getting Started](./getting-started), [Sites](./sites), [HTTPS & Certificates](./https)
