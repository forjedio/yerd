# Security Policy

We take the security of Yerd seriously. Yerd binds local ports, manages TLS
certificates and a local trust store, and elevates privileges once during setup
(via the `yerd-helper` boundary), so we appreciate reports that help keep it
safe.

## Supported versions

Yerd is pre-1.0 and ships as a rolling release. Security fixes land on the
latest release; please reproduce against the most recent version before
reporting.

## Reporting a vulnerability

**Please do not open a public GitHub issue for security-sensitive reports.**

For any issue you consider sensitive, email **security@forjed.io**. Include:

- a description of the vulnerability and its impact;
- the Yerd version (`yerd --version`) and platform (macOS / Linux);
- steps to reproduce, or a proof of concept, if you have one;
- any relevant logs (daemon or GUI), with secrets redacted.

You can also use GitHub's
[private vulnerability reporting](https://github.com/forjedio/yerd/security/advisories/new)
if you prefer to disclose through GitHub.

For non-sensitive, low-risk issues (e.g. a hardening suggestion with no
exploit), a regular [bug report](https://github.com/forjedio/yerd/issues/new/choose)
is fine.

## What to expect

- We'll acknowledge your report as soon as we can.
- We'll investigate, keep you updated on our assessment, and let you know when a
  fix ships.
- We're happy to credit you in the release notes once the issue is resolved -
  let us know how you'd like to be named, or if you'd prefer to stay anonymous.

Please give us a reasonable opportunity to release a fix before any public
disclosure. Thank you for reporting responsibly.
