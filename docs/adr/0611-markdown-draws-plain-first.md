# 0611: Markdown draws as escaped plain text on a cold open and formats once the lazily loaded markdown engine lands; amends 0288 and 0436

- **Status**: Accepted
- **Date**: 2026-11-01

## Context

`vite build` warned on both first-paint budgets (ADR 0436). The entry chunk
measured 557.2 kB against its 540 kB soft line. The first-paint set measured
1334.9 kB against its 1260 kB soft line. The goal was to clear both warnings
without raising a number, and to load faster.

Three levers moved bytes off the first frame:

1. **Build for ES2022.** Vite's default target lowered syntax that every
   platform we support already runs natively.
2. **The drawer order handle.** The data layer read the drawer order through
   the thread family graph, which pulled that graph into the entry chunk.
3. **The markdown engine.** `marked`, DOMPurify and our renderer sat in the
   first-paint set, because the transcript renders markdown in its first frame.

The third lever contradicts ADR 0288. That ADR keeps in the first frame what a
cold open can draw. A cold open onto a thread draws markdown at once.

## Decision

The **markdown engine** (`utils/markdownEngine.ts`, `marked` and DOMPurify)
loads behind a dynamic import. `main.tsx` requests it right after the shell
chunk, under the same gate. First-paint code reaches it only through the
facade, `utils/renderMarkdown.ts`. A chunk that is already lazy may import the
engine directly.

Until the engine lands, the facade returns the text HTML-escaped, with line
breaks kept, and caches nothing. Once it lands, the `markdownEngine` signal
changes, and every memoised caller renders again, formatted.

This amends ADR 0288: what a cold open draws stays in the first frame, *unless
it has an honest plain form*. Markdown has one. A permission card does not.

## Rationale

**Plain text is a real first frame, not a flash.** Escaped text shows every
word in place. Formatting arrives as a restyle of the same content, usually
before the boot splash lifts. A missing card would be a hole in the page.

**The engine load runs beside the startup fetches.** It starts as soon as the
entry evaluates, like the shell chunk. So on a warm service worker cache it
lands within the splash, and the plain form shows only on a slow network.

**A failed load takes the stale-chunk path.** `chunkLoader` already handles a
chunk a new build removed. The markdown engine uses the same path, so a stale
tab reloads rather than staying plain.

**The facade keeps the split checkable.** No first-paint module imports the
engine, `marked` or DOMPurify. A test walks the entry and shell graphs and
fails if either reaches them. Marker constants and `escapeHtmlAttr` moved into
small modules of their own, so first-paint code can share them.

## Consequences

Measured with the dev build-watch signal, raw kB of 1000 bytes:

| Build | Entry | First-paint set |
|---|---|---|
| main before this change | 557.2 | 1334.9 |
| after ES2022 and the drawer order handle | 538.76 | 1310.05 |
| after the markdown engine | 539.37 | 1227.18 |

- `vite build` prints no budget warning. The set sits 32.8 kB under its soft
  line, past ADR 0436's 25 kB margin.
- **The entry has almost no honest headroom left.** It sits 0.63 kB under its
  soft line. The facade and the engine loader cost about 0.6 kB of entry.
  Almost everything else in the entry runs at boot, and ADR 0288 rejected
  splitting the store. The next entry growth will warn.
- The lazy chunks are `marked` (`marked` and DOMPurify, 71.66 kB) and
  `markdownEngine` (11.96 kB).
- A memoised caller of `renderMarkdown` must list `markdownEngine.value` in its
  dependencies. Without it, the caller keeps the plain form after the engine
  lands.
- Tests provide the engine in `src/test-setup.ts`, so a component test renders
  formatted markdown as before.
- Lazy surfaces that need the sanitizer directly import `markdownEngine.ts`.
  That is safe only from a chunk that is already lazy.

## Alternatives considered

- **Raise the soft lines.** Rejected, as in ADR 0288 and ADR 0436: the numbers
  are ceilings.
- **Keep markdown in the first frame and split the store instead.** ADR 0288
  measured it: the store is densely connected and runs at boot.
- **Precompress with Brotli.** It shrinks the download only. The budgets count
  raw bytes, because parse and evaluate recur on every cold open.
- **Hold the boot splash until the engine lands.** It would trade a restyle for
  a longer splash on every slow open.
