---
paths:
  - "packages/lucidos-sdk/**/*.ts"
  - "packages/lucidos-sdk/**/*.mjs"
---

# Lucidos JS SDK

`packages/lucidos-sdk/` is the source for `window.lucidos.*` — the surface apps load via `<script src="/api/v1/sdk.js">`. The user-facing contract for that surface lives at `system-knowhow/js-sdk.md`. Treat `js-sdk.md` as the spec; the code in this package implements it.

## Before adding a method

1. **Check it doesn't already exist.** Each top-level namespace has a sibling source file (`data.ts`, `events.ts`, `proxy.ts`, `preferences.ts`, `notifications.ts`, `apps.ts`, `threads.ts`, `triggers.ts`, `ui.ts`, `sse.ts`, `utils.ts`, `capture.ts`). `Grep pattern: 'lucidos\\.<methodname>'` over the repo (or just look for the matching `## lucidos.<namespace>` heading in `system-knowhow/js-sdk.md`). The existing helper may be lower-level than the new spec wants — say so explicitly to the user instead of silently shipping a duplicate.
2. **Read the matching `## lucidos.<namespace>` section in `system-knowhow/js-sdk.md`.** It documents the existing signature, examples, and "When to use which" tables that callers already rely on.

## When you change the surface

- **Update `system-knowhow/js-sdk.md` in the same commit.** Apps and audit knowhow link into specific `§ lucidos.<name>` headings — the heading is part of the contract, not just docs.
- **Add the new symbol to `src/index.ts`** (`import { ... } from './<file>'` and into the `lucidos = { ... }` object). The IIFE bundle pulls from `index.ts` via `browser.ts`, and the frontend ES-imports from `index.ts` directly. A new file that isn't re-exported reaches neither.

## Build & runtime

- Bundle: `cd packages/lucidos-sdk && npm run build` (esbuild → `dist/sdk.js`, IIFE, sourcemap).
- Served by the engine at `/api/v1/sdk.js` (debug builds re-read every request; release caches once on first request, so a restart is needed). The engine restart trigger lives in `crates/lucidos-engine/src/engine/git_ops/restart_detection.rs` (`files_require_restart`); any change under `packages/lucidos-sdk/` already triggers it.
- Internal helpers (`_fetch.ts`, `_validate.ts`) are leading-underscore — not part of the public surface, don't reference them from `js-sdk.md`.

## Tests

`*.test.ts` next to source, run by Vitest from the workspace root. Pure functions only — no DOM-dependent SDK logic has tests today (see `scroll.test.ts` for the established style).
