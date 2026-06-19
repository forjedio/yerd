# yerd-mail

`yerd-mail` is Yerd's built-in **mail-capture SMTP sink** plus the **on-disk
store** behind it. Herd-style: the daemon runs a tiny SMTP server on a loopback
port, writes everything it receives as a raw `.eml` file, and surfaces each
message (decoded) to the GUI and CLI for inspection. There is no relaying -
**captured mail never leaves the box**.

The crate is consumed by [`yerdd`](../binaries/yerdd) (the long-running daemon),
which opens the store at startup, optionally binds the listener, and answers the
mail [IPC requests](./yerd-ipc) by reading and mutating the store.

::: info Crate metadata
`description`: *Built-in mail-capture SMTP server + on-disk store for Yerd
(Herd-style).* `#![forbid(unsafe_code)]`. Depends on [`yerd-ipc`](./yerd-ipc) (for
the owned wire types `MailSummary` / `MailDetail` / `MailHeader`) and
[`mail-parser`](https://github.com/stalwartlabs/mail-parser) - a pure-Rust MIME
parser with no C dependencies. The only async runtime is `tokio`.
:::

See also the [Crates overview](../crates), [`yerd-ipc`](./yerd-ipc) (the request /
response types), and the user-facing [Mail Capture guide](../../guide/mail).

## Module map

The crate mirrors the `pure` / `io` split used across the Yerd workspace: all
*decisions* (the SMTP command state machine, MIME decoding, retention policy) are
synchronous and I/O-free; all *effects* (the TCP server, the disk store) sit in a
thin I/O layer.

```text
src/
├── lib.rs            # re-exports + the purity-boundary doc
├── error.rs          # MailError (Bind / Io / Index)
├── pure/
│   ├── smtp.rs       # Session - the SMTP receiver state machine + RawMessage
│   ├── mime.rs       # summary() / detail() - decode an .eml into owned wire types
│   └── retention.rs  # evict_count() + DEFAULT_CAP
└── io/
    ├── server.rs     # bind() / serve() - the tokio SMTP accept loop
    └── store.rs      # Store - the on-disk .eml store + index.json cache
```

[Browse the source on GitHub.](https://github.com/forjedio/yerd)

## Public API

Everything is re-exported from `lib.rs`:

```rust
pub use error::MailError;
pub use io::server::{bind, serve};
pub use io::store::Store;
pub use pure::smtp::RawMessage;
```

| Item | Layer | Role |
|------|-------|------|
| `bind(port)` | io | Bind the loopback SMTP listener; returns a `TcpListener`. |
| `serve(listener, store, shutdown)` | io | Accept loop: drive a `Session` per connection and persist each message. |
| `Store` | io | The on-disk `.eml` store + `index.json` metadata cache. |
| `RawMessage` | pure | One captured message: envelope + verbatim dot-unstuffed body. |
| `MailError` | - | The crate's error type. |

## The `pure/` layer

Everything under `pure/` is synchronous, I/O-free, and unit-testable without
sockets or a filesystem. `smtp.rs` owns no sockets; `mime.rs` takes a byte slice
and returns owned types; `retention.rs` is a single arithmetic function.

### `smtp` - the SMTP receiver state machine

`Session` speaks just enough of RFC 5321 to capture mail from a local app's
mailer: `EHLO`/`HELO`, `MAIL FROM`, `RCPT TO`, `DATA`, `RSET`, `NOOP`, `QUIT`.
There is **no AUTH, no TLS, and no relaying** - every recipient is accepted and the
body is captured verbatim. The module owns no I/O: the server reads a line, calls
`Session::command`, and acts on the returned `Reply`.

```rust
pub fn command(&mut self, line: &str) -> Reply;
pub fn finish_data(&mut self, data: &[u8]) -> RawMessage;
```

`Reply` tells the I/O layer what to do next:

```rust
pub enum Reply {
    Line(String),       // write this reply, keep reading commands
    StartData(String),  // write this reply, then collect DATA until \r\n.\r\n
    Close(String),      // write this reply, then close (QUIT)
}
```

The command surface and the replies it emits:

| Verb | Reply | Notes |
|------|-------|-------|
| `HELO` / `EHLO` | `250 yerd` | Greeting on connect is `220 yerd mail capture ready`. |
| `MAIL` | `250 OK` | Begins a new transaction: clears any leftover recipients so an abandoned prior envelope can't leak into this message. |
| `RCPT` | `250 OK` | Address pushed in order; all recipients accepted. |
| `DATA` | `354 …` (`StartData`) or `503 RCPT first` | Refuses with `503` when no recipient has been given. |
| `RSET` | `250 OK` | Clears `from` and recipients. |
| `NOOP` | `250 OK` | |
| `QUIT` | `221 Bye` (`Close`) | |
| empty line | `500 Syntax error` | |
| anything else | `250 OK` | **Lenient catch-all**: a capture sink accepts whatever a dev mailer sends. |

`extract_address` pulls the address from between the angle brackets of
`MAIL FROM:<addr>` / `RCPT TO:<addr>`, falling back to the trimmed text after the
first `:` when there are no brackets.

`finish_data` is fed the bytes between the `354` and the terminating `\r\n.\r\n`
(that terminator already stripped by the server). It **dot-unstuffs** them via the
pure `unstuff` helper - undoing SMTP's RFC 5321 §4.5.2 transparency, where a body
line beginning with `.` is sent with an extra leading `.` - and returns a
`RawMessage`:

```rust
pub struct RawMessage {
    pub envelope_from: String,  // MAIL FROM address
    pub recipients: Vec<String>, // RCPT TO addresses, in order
    pub raw: Vec<u8>,            // dot-unstuffed body: an RFC 5322 message
}
```

`finish_data` also resets the envelope, so the same connection may send another
message.

### `mime` - decoding a captured `.eml`

`mail-parser` is a zero-copy parser - its output borrows the input via `Cow` and
lifetimes - so nothing it returns can cross the IPC wire or be stored directly.
This module clones every field out into owned `String` / `u64` values, producing
the owned [`yerd-ipc`](./yerd-ipc) wire types:

```rust
pub fn summary(id: &str, raw: &[u8]) -> MailSummary;  // metadata only
pub fn detail(id: &str, raw: &[u8]) -> MailDetail;    // headers + decoded bodies
```

- **`summary`** decodes only the envelope metadata (`from`, `to`, `subject`,
  `date_epoch`) - cheap, used to build the index entry on capture.
- **`detail`** decodes the full content: all header lines (sliced byte-exact from
  the raw message and UTF-8-lossy trimmed), the decoded text body, and the decoded
  HTML body.

Two decoding subtleties, both grounded in the source:

- **Genuine HTML only.** `mail-parser`'s `body_html` would *synthesise* HTML from a
  text-only message. `detail` only surfaces `html_body` when a real `text/html`
  part is present (checked via `is_html_part`), so a text-only message leaves
  `html_body` as `None` and a client falls back to `text_body`. This is what lets
  `yerd mail show` print "(HTML-only message …)" only when there genuinely is no
  text part.
- **`cid:` → `data:` rewrite.** When an HTML body references inline attachments by
  `cid:`, `rewrite_cids` replaces each `cid:<id>` (and `CID:<id>`) with an inline
  `data:<mime>;base64,<…>` URL built from the matching attachment, so a sandboxed
  viewer renders embedded images **without any network access**. A small local
  standard-alphabet `base64_encode` is inlined here rather than pulling a base64
  dependency for this one use.

### `retention` - bounding the store

```rust
pub const DEFAULT_CAP: usize = 200;
pub fn evict_count(current_len: usize, cap: usize) -> usize;
```

`evict_count` is a one-liner - `current_len.saturating_sub(cap)` - returning how
many of the **oldest** entries must be evicted to get back within the cap (zero
when already within bounds, assuming oldest-first ordering). The `Store` calls it
after every append.

## The `io/` layer

These edges do socket and filesystem work and are therefore deliberately outside
`pure/`.

### `server` - the tokio SMTP capture server

```rust
pub async fn bind(port: u16) -> Result<TcpListener, MailError>;
pub async fn serve<S>(listener: TcpListener, store: Arc<Store>, shutdown: S)
    -> Result<(), MailError>
where S: Future<Output = ()> + Send + 'static;
```

`bind` binds `127.0.0.1:<port>` (loopback only - never a routable address),
surfacing a bind failure as `MailError::Bind` (which the daemon treats as
non-fatal). `serve` is the accept loop: it `tokio::select!`s the `shutdown` future
against `listener.accept()` (`biased`, so shutdown wins), and `tokio::spawn`s a
per-connection task that drives the pure `Session`.

Per connection, `handle_conn`:

1. writes the `220` greeting,
2. reads command lines, feeding each to `Session::command` and writing the
   `Reply`,
3. on `StartData`, reads the body via `read_data` until the `\r\n.\r\n` marker,
   calls `Session::finish_data`, and `store.append`s the raw bytes - replying
   `250 OK: queued` on success or `451 storage error` if the store write failed.

`read_data` enforces a defensive `MAX_MESSAGE_BYTES` cap (25 MiB). Once a message
goes oversized it stops *appending* but keeps *consuming* lines until the
terminating dot, so a truncated body is never re-read as SMTP commands (which would
desync the connection for any subsequent message).

::: info Best-effort by design
A per-connection error is only logged at `debug`; a failed store write surfaces to
the client as `451` but never tears down the server. Mail capture is meant to be a
convenience that can't take your sites down.
:::

### `store` - the on-disk store

`Store` is the persistent capture store; the daemon holds it behind an `Arc`.
Layout under the store directory:

- `<id>.eml` - the verbatim captured message, one per email.
- `index.json` - an ordered, **oldest-first** list of `MailSummary` metadata, so
  listing doesn't re-parse every `.eml`.

```rust
pub fn open(dir: PathBuf) -> Result<Self, MailError>;     // DEFAULT_CAP
pub fn open_with_cap(dir: PathBuf, cap: usize) -> Result<Self, MailError>;
pub async fn append(&self, raw: &[u8]) -> Result<(), MailError>;
pub async fn list(&self) -> Vec<MailSummary>;             // newest-first
pub async fn count(&self) -> u32;
pub async fn get(&self, id: &str) -> Result<Option<MailDetail>, MailError>;
pub async fn delete_many(&self, ids: &[String]) -> Result<(), MailError>;
pub async fn clear(&self) -> Result<(), MailError>;
```

Design properties worth knowing as a contributor:

- **Single mutex, no file locks.** All mutations go through one
  `tokio::sync::Mutex<Inner>`, so concurrent SMTP connections appending at once
  can't lose an index update. Advisory file locks / `fs2` are **forbidden by the
  workspace dep-graph gate**, hence the in-process mutex.
- **Monotonic, never-reused ids.** Ids are a zero-padded (`{:06}`) counter that
  sorts in receipt order and is never reused - not even across `clear` /
  `delete_many`, which delete files but don't reset the counter. On `open`, the
  counter is seeded from the max of *both* the index *and* any `<id>.eml` on disk
  (`max_eml_id`), so an `.eml` written but never recorded in the index (a crash
  between the two writes) can never have its id reused.
- **Cap eviction on append.** After writing the new `.eml` and pushing its
  `summary`, `append` calls `retention::evict_count` and removes the oldest
  entries' `.eml` files beyond the cap.
- **Newest-first reads, oldest-first storage.** `entries` is stored oldest-first
  (cheap eviction from the front); `list` reverses it so callers see newest-first.
- **Atomic index writes.** `write_index` writes a sibling `index.json.tmp` then
  renames it over `index.json` - the same write-temp-then-rename discipline as
  [`yerd-config`](./yerd-config) / [`yerd-php`](./yerd-php), so a crash or partial
  write can never leave a truncated index. Rename is atomic on the same filesystem.
- **Corrupt index is recoverable, not fatal.** `load_index` treats garbled JSON as
  recoverable: the `.eml` files are the source of truth and `max_eml_id` reseeds
  the counter, so a corrupt index logs a warning and starts from empty rather than
  failing `Store::open` (which would take down the whole daemon). An absent
  `index.json` is simply an empty store.

## Error model

`MailError` (`#[non_exhaustive]`, `thiserror`) has three variants:

| Variant | Meaning |
|---------|---------|
| `Bind { port, source }` | The loopback SMTP port couldn't be bound (e.g. in use). The daemon logs and runs with capture **not listening**. |
| `Io { path, source }` | A filesystem operation on the store failed (carries the path for diagnostics). |
| `Index(serde_json::Error)` | The `index.json` cache couldn't be (de)serialised. |

It is intentionally **not** `Clone`/`Eq` because it wraps `std::io::Error` and
`serde_json::Error`.

## How `yerdd` consumes it

At startup the daemon ([`bin/yerdd/src/startup.rs`](../binaries/yerdd)):

1. **Always opens the store** at `<data>/mail` via `Store::open`. The store exists
   even when capture is disabled, so already-captured mail stays listable and
   clearable after the server is turned off.
2. **Binds the listener only when `[mail].enabled`** is true, calling
   `yerd_mail::bind(port)` with `config.mail.port` (default
   [`DEFAULT_MAIL_PORT`](./yerd-config) = `2525`). A bind failure is logged and
   degrades to non-listening - **non-fatal**. The resulting `listening` flag is
   recorded in `MailRuntime` and surfaced in `Status`.
3. **Spawns `serve`** with the bound listener, an `Arc<Store>` clone, and the
   daemon's shutdown future.

The IPC server then maps the mail [requests](./yerd-ipc) onto the store:

| Request | Store call |
|---------|-----------|
| `ListMails` | `store.list()` → `MailSummary`s, newest-first |
| `GetMail { id }` | `store.get(&id)` → `Option<MailDetail>` |
| `ClearMails` | `store.clear()` |
| `DeleteMails { ids }` | `store.delete_many(&ids)` (e.g. all mail for one app) |
| `SetMailPort { port }` / `SetMailEnabled { enabled }` | persist to the `[mail]` config table |

::: warning Port / enabled changes need a restart
`SetMailPort` and `SetMailEnabled` save to config immediately but take effect on
the **next daemon start/restart** - whether the server is actually bound is a
startup property, with no implicit hot rebind. This mirrors `SetServicePort`.
:::

## See also

- [Mail Capture guide](../../guide/mail) - the user-facing feature
- [Mail CLI reference](../../reference/cli/mail) - `list` / `show` / `clear`
- [yerd-ipc](./yerd-ipc) - the `MailSummary` / `MailDetail` wire types and requests
- [The Daemon](../../guide/daemon) - the binder and consumer of this crate
- [Crates Overview](../crates)
- Source: [`crates/yerd-mail`](https://github.com/forjedio/yerd/tree/main/crates/yerd-mail)
