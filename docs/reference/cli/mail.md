# Mail

The `yerd mail` commands inspect emails captured by Yerd's built-in mail-capture
SMTP server. The server runs inside the [`yerdd` daemon](../../guide/daemon),
enabled by default on `127.0.0.1:2525`; point an app's mailer at it and every
message it sends is stored locally for inspection. The [Mail Capture
guide](../../guide/mail) covers the model and how to wire an app up; this page is
the command reference.

::: info Captured mail is local-only
The capture server is a sink, not a relay - nothing is ever forwarded. Stored mail
stays listable and clearable even when capture is disabled, so these commands work
regardless of whether the listener is currently bound.
:::

## Commands

| Command | Description |
| --- | --- |
| `yerd mail list` | List captured emails (newest first). |
| `yerd mail show <ID>` | Show one captured email's headers and body. |
| `yerd mail clear` | Delete every captured email. |

```sh
yerd mail list
yerd mail show 000003
yerd mail clear
```

## `yerd mail list`

Prints a tab-separated table of every captured email, **newest first**, with three
columns: `ID`, `FROM`, and `SUBJECT`.

```sh
yerd mail list
# ID      FROM                        SUBJECT
# 000003  Example <hi@example.com>    Password Reset
# 000002  shop@acme.test              Your order shipped
# 000001  no-reply@acme.test          Welcome
```

- The `ID` is a zero-padded, monotonic counter (the on-disk `<id>.eml` stem). Ids
  are never reused - not even after `clear` - so an id always refers to the same
  message for as long as it's retained.
- A message with no subject is shown as `(no subject)`.
- Tabs, carriage returns, and newlines inside a header are flattened to spaces so a
  folded or multi-line header can't break the table.
- When nothing has been captured, the command prints `no captured emails`.

## `yerd mail show <ID>`

Shows one captured email by id. The `<ID>` is the value from the `ID` column of
`yerd mail list`.

```sh
yerd mail show 000003
# From:    Example <hi@example.com>
# To:      you@app.test
# Subject: Password Reset
#
# Your OTP is 416063.
```

Output is the `From:`, `To:`, and `Subject:` headers, a blank line, then the
message's decoded **text body**. The bodies are already MIME-decoded (charset and
transfer-encoding handled).

| Message content | What `show` prints |
| --- | --- |
| Has a plain-text body | The decoded text body. |
| HTML only (no text part) | `(HTML-only message — open it in the GUI viewer)` |
| Neither body | `(empty message)` |

::: tip HTML bodies render in the desktop app
The CLI deliberately doesn't dump raw HTML. To read an HTML-only message - with
inline images and all - open it in the **Mail** view of the [desktop
app](../../guide/desktop-app), which renders it in a sandboxed viewer.
:::

## `yerd mail clear`

Deletes **every** captured email and prints `ok`.

```sh
yerd mail clear
```

This removes all stored `.eml` files. The id counter is **not** reset, so a later
capture never reuses the id of a cleared message.

::: warning No per-message delete from the CLI
`yerd mail clear` is all-or-nothing. To remove individual messages, use the
**Mail** view in the [desktop app](../../guide/desktop-app).
:::

## JSON output

Every command accepts the global `--json` flag for machine-readable output:

```sh
yerd mail list --json          # array of captured-email metadata
yerd mail show 000003 --json   # one email's full decoded content
```

`yerd mail list --json` emits the captured-email metadata (`id`, `from`, `to`,
`subject`, `date_epoch`). `yerd mail show <id> --json` emits the full decoded
message, including all `headers` and both the `html_body` and `text_body`
(whichever the message carries). `date_epoch` is the message `Date:` as Unix epoch
seconds, or `0` when absent/unparseable.

## See also

- [Mail Capture guide](../../guide/mail) - what capture is and how to point an app at it
- [Configuration Reference](../configuration) - the `[mail]` table (`enabled`, `port`)
- [yerd-mail](../../developer/crates/yerd-mail) - the crate behind these commands
