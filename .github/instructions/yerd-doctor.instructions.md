---
applyTo: "crates/yerd-doctor/**/*.rs"
---

# yerd-doctor — diagnosis & fix planning

Pure diagnosis and fix-planning for `yerd doctor`.

**Layer:** strictly pure and runtime-free. Depends only on `yerd-core` and
`yerd-ipc` types. Crate root carries `#![forbid(unsafe_code)]`.

## Owns

- `diagnose(&StatusReport) -> Vec<Diagnosis>`: turns a daemon status report into
  typed findings with severity.
- `plan_auto_fixes(&StatusReport) -> Vec<FixAction>`: the safe, unprivileged
  fixes the daemon may apply automatically.

## Must not

- Perform any I/O. The daemon assembles the `StatusReport`, applies fixes, and
  re-runs `diagnose` afterwards — this crate only decides.
- Plan fixes from a wire `Diagnosis` (strings only). Plan from the typed
  `StatusReport` so a `FixAction` can carry the precise typed value (e.g. a
  `yerd_core::PhpVersion`).
- Plan a fix that needs elevation as an "auto" fix — auto-fixes are
  unprivileged and safe only.

## Conventions

- `FixAction` is `#[non_exhaustive]`; add variants additively.
- Treat the privileged-port ceiling and similar thresholds as named constants,
  not magic numbers.

## Review checklist

- [ ] No I/O; decisions are derived purely from the typed report.
- [ ] Auto-fixes are unprivileged and safe; privileged remediation is left to
      manual/daemon paths.
- [ ] New findings/actions are table-tested and additively introduced.
