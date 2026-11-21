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

- **A scheme the browser can act on** (`http:`, `https:`, `mailto:`, `tel:`,
  `sms:`). A real link. `file:` and absolute disk paths never reach the guard,
  being claimed above by the OS opener.
- **A pure fragment** (`#section`). An in-page markdown anchor navigates
  nothing.

**Amended.** This first read "a URL scheme", any of them, which is a wider
exemption than the reasoning supports. A scheme nothing claims is not a real
link: clicked, it does nothing and reports nothing, which is the failure this
ADR exists to close, in its worst form. The agent inventing `trigger:<id>` was
the report that showed it (ADR 0112). The exemption is now the enumerated set
above, `browserHandlesHref` in `utils/linkifyPaths.ts`. Everything else falls
into the guard and toasts.

**A deliberate markdown link to a `data/` path is resolved by SHAPE, not by
cache membership.** `extractDataPathTarget` recognises the `data/` sub-trees and
routes to the file preview whether or not the artifact list knows the path. A
path that turns out not to exist surfaces as the preview's own 404, which is
recoverable. Concluding "no such file" from an SSE-refreshed projection is the
stale-cache failure `.claude/rules/frontend.md` already bans elsewhere.

**Amended 2026-08-21: the text-segment linkifier resolves by shape too.** This
decision originally limited shape resolution to **anchors**, on the reasoning
that matching a path shape in prose would linkify every incidental mention. That
was too strict, and it left the same stale-cache hole one layer down.

The chat system prompt *instructs* the agent to write a bare full path, telling
it that a full path becomes a link. A full path in a reply is therefore as
deliberate as an anchor. Gating it on cache membership broke a promise the
workspace itself made. It surfaced as four video paths rendering flat: each had
been placed under `data/artifacts/` by a shell `cp` inside `run_bash`, which
announces nothing, so the cached list had never heard of them.

What makes prose safe is one guard an anchor does not need: **the final segment
must carry an extension**. A directory the agent mentions in passing
(`artifacts/marketing`) stays plain, which is most of what "incidental mention"
meant. Two further limits hold the line. A bare filename with no sub-tree prefix
(`notes.md`) stays list-gated, because shape cannot tell it from an ordinary
word. And a match preceded by a word character or `/` is rejected, so the path
half of a URL is never carved out.

Rationale and the full invariant set:
`docs/plans/2026-08-21-a-bare-data-path-in-prose-links-by-shape.md`.

**Amended 2026-08-27: an app link may carry a fragment.** The inventory above
lists the shapes that reached the guard, and an app entry point now has one more
form: a trailing `#frag` naming a place inside the app it opens. It is the *app
fragment* (`docs/glossary.md`), delivered to the iframe as `location.hash`.

| App link shape | Example |
|---|---|
| `app:<id>` custom scheme | `[Habit Tracker](app:habit-tracker)` |
| `apps/<id>[/index.html]` path | `[Habit Tracker](apps/habit-tracker/index.html)` |
| either, with a fragment | `[Some report](app:pr-understanding#pr-1645)` |

Nothing about the guard changes. The same extractor claims the same hrefs, and
`app:` is still claimed before the terminal guard sees it. The bare-app-ref
shapes carry no fragment, being model-tolerance measures.
Rationale: `docs/plans/2026-08-27-app-links-carry-a-fragment.md`.

## Consequences

- The next unrecognised href shape is a toast the user can read and report, not
  a workspace reload. Adding an extractor for it becomes an improvement rather
  than a bug fix.
- **A dead link now says so.** Previously the workspace reloaded and the user
  was left guessing whether the link had "worked".
- Hrefs that used to escape now resolve somewhere. `apps/<id>/styles.css`
  previews as a file. `apps/<unknown-id>/index.html` routes through the app
  opener instead, which re-fetches the registry before reporting the app
  gone. The app gate stays strict: only an id that genuinely exists opens
  one.
- The guard has to stay last. A new extractor added after it would be dead code
  and would read as covering a shape the guard already ate, so a source-scan
  test pins its position and its two exemptions.
- **A third-party scheme is now swallowed too** (`vscode:`, `zoommtg:`). It
  would have worked on a machine with that handler registered, so this is a
  real cost, paid to close the silent-failure class. Adding one back is a line
  in `BROWSER_NAVIGABLE_SCHEMES`.
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
