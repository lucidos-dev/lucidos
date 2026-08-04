# 0038: A chat link never leaves the workspace

- **Status**: Accepted
- **Date**: 2026-08-04

## Context

Chat responses are markdown, and the model writes links in them. The frontend
rewrites the ones it recognises into click-routed anchors (`.artifact-link`,
`.app-link`, `.nav-link`) and `ChatExchange.handleLinkClick` routes the rest
through a chain of href extractors. Anything no extractor claims is left to the
browser.

Leaving it to the browser is not neutral. The SPA has no relative routes: a
scheme-less href resolves against the workspace base, the engine's SPA fallback
answers any unmatched path with the app shell, and the whole workspace reloads.
On an installed iOS PWA that is the full "Opening workspace" splash. Work in
progress in the compose box, scroll position, and open panels all go.

The same bug was reported five times, once per href shape the model invented:

| Shape | Example |
|---|---|
| `app:<id>` custom scheme | `[Habit Tracker](app:habit-tracker)` |
| bare app id or name | `[Habit Tracker](habit-tracker)` |
| nav panel under `data/` | `[Notifications](data/notifications)` |
| bare `app`, no id | `[Site Publisher](app)` |
| a `data/` file path | `[report](artifacts/pr-review/pr-1582/index.html)` |

Each was fixed by adding one more extractor. Each fix was correct and none of
them was the fix, because the defect is not in any extractor: it is that the
chain ends in an implicit "let the browser have it". A whitelist is open at the
bottom, and the model's link vocabulary is not a closed set we can enumerate
ahead of it.

The fifth report exposed a second edge. The artifact rewriter resolved an href
against the CACHED artifact list, so a file written seconds earlier (by
`lucidos data write`, which prints exactly that link shape for the agent to
paste) was not in the cache, and a link the workspace itself had just generated
was treated as unrecognised.

## Decision

**No anchor click inside a chat exchange performs a top-level navigation.**
`handleLinkClick` ends in a terminal guard: any href that reaches the bottom of
the chain unclaimed is `preventDefault()`ed and reported as a toast naming it.
Two things pass through, both deliberately:

- **A URL scheme** (`https:`, `mailto:`, `tel:`, `file:`). A real link, and
  `file:` / absolute disk paths are already claimed above by the OS opener.
- **A pure fragment** (`#section`). An in-page markdown anchor navigates
  nothing.

**A deliberate markdown link to a `data/` path is resolved by SHAPE, not by
cache membership.** `extractDataPathTarget` recognises the `data/` sub-trees and
routes to the file preview whether or not the artifact list knows the path. A
path that turns out not to exist surfaces as the preview's own 404, which is
recoverable. Concluding "no such file" from an SSE-refreshed projection is the
stale-cache failure `.claude/rules/frontend.md` already bans elsewhere.

Shape-based resolution is limited to **anchors**. The text-segment linkifier,
which scans prose for bare paths, stays list-gated: matching a path shape there
would linkify every incidental mention.

## Consequences

- The next unrecognised href shape is a toast the user can read and report, not
  a workspace reload. Adding an extractor for it becomes an improvement rather
  than a bug fix.
- **A dead link now says so.** Previously the workspace reloaded and the user
  was left guessing whether the link had "worked".
- Hrefs that used to escape now resolve somewhere. `apps/<unknown-id>/index.html`
  and `apps/<id>/styles.css` preview as files instead of navigating away; the
  app gate stays strict, so only a KNOWN id ever opens an app.
- The guard has to stay last. A new extractor added after it would be dead code
  and would read as covering a shape the guard already ate, so a source-scan
  test pins its position and its two exemptions.
- **Not covered: an absolute same-origin URL.** `[x](https://<host>/artifacts/y)`
  carries a scheme, so it passes through to the browser like any external link.
  Bare URLs the linkifier finds in prose get `target="_blank"` and open a tab;
  a markdown-authored one does not, and would navigate the tab. Left alone: it
  is indistinguishable from a deliberate external link, and the model does not
  write it.

## Related

- ADR 0032 (a state write owns its announcement) is the other half of the fifth
  report. `lucidos data write` wrote `data/` directly and announced nothing, so
  the cache it needed to warm was never refreshed. It now routes through the
  engine's announced write path. ADR 0032's registry covers `data/` writers in
  the engine crate, so it could not have caught a writer in a separate binary.
