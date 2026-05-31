---
applyTo: "apps/yerd-gui/src/**/*.{vue,ts}"
---

# yerd-gui frontend — Vue 3 + TypeScript + Tailwind

The desktop UI. It is a **client** that talks to the daemon through Tauri
commands; it holds view state, not product state.

**Stack:** Vue 3 with `<script setup>`, TypeScript, Tailwind. Tests with
`vitest` + `@vue/test-utils`.

## Conventions

- Use `<script setup lang="ts">` for components. Type props and emits
  explicitly; avoid `any`. Prefer composables (`composables/`) for shared
  reactive logic over duplicating it across components.
- All daemon interaction goes through the IPC layer (`src/ipc/`) which invokes
  the Tauri commands. Components do not call `invoke` ad hoc or assume a daemon
  response shape outside the typed IPC client.
- Keep the wire types in `src/ipc/types.ts` aligned with the Rust `yerd-ipc`
  contract. When the protocol gains a field/variant, extend the TS types to
  match rather than casting around them.
- Styling via Tailwind utility classes and the shared design tokens in
  `style.css`; reuse existing UI components in `components/ui/` before adding new
  primitives.

## Must not

- Reimplement daemon logic (routing, supervision, config decisions) in the
  frontend. The UI renders state and issues requests; it does not own truth.
- Assume privileged capability — elevation is the daemon/helper's job, surfaced
  to the user, never performed by the UI.
- Leave features silently broken: stubbed/"coming soon" capabilities should read
  as such, not appear functional.

## Tests

- Component and composable logic is unit-tested with `vitest`. A change is
  verified locally with `npm run test` and a clean `npm run build` (which runs
  `vue-tsc --noEmit` — type errors fail the build).

## Review checklist

- [ ] `<script setup lang="ts">`, typed props/emits, no stray `any`.
- [ ] Daemon access goes through the typed IPC layer; TS types match the Rust
      contract.
- [ ] Shared logic extracted into composables; existing UI primitives reused.
- [ ] `npm run test` and `npm run build` pass; no logic duplicated from crates.
