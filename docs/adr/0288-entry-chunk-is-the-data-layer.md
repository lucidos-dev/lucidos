# 0288: The entry chunk holds the data layer and startup only; the UI shell is a preloaded parallel chunk the splash covers, and interaction-only popovers are idle-prefetched

- **Status**: Accepted
- **Date**: 2026-09-26

## Context

The `crates/lucidos-app` entry chunk reached 858 kB against its own 600 kB
`chunkSizeWarningLimit`, and grew about 20 kB a night. Rollup's advisory only
prints, so every build exited 0 while the nightly clean-build gate failed.

Sourcemap attribution showed no quick lever. The chunk held zero
`node_modules` bytes, no module sat in two chunks, and no `import()` target had
leaked in. Growth was diffuse across about 440 of our own modules. The seven
on-demand surfaces the clean-build skill had measured added up to 40 kB against
a 258 kB gap.

Two facts in the code decided the fix:

- **The user never sees the app's first render.** The inline boot splash lifts
  only when the engine is connected and the thread list has loaded. On a fresh
  launch it also holds for 1.2 s. That is true on desktop and on an iOS PWA
  cold open.
- **Every startup fetch waited for the whole UI.** They ran in `useStartup`, a
  hook `<App/>` called. So nothing reached the network until all 858 kB had
  downloaded, parsed, evaluated and rendered once.

## Decision

The entry chunk is the data layer and startup: store, actions, event stream,
API client, SDK, and the diagnostic probes. `boot()` calls `startClient()`
(`store/startup.ts`) before it renders anything.

The UI is the **shell chunk**: `main.tsx` imports `<App/>` lazily and asks for
it as soon as the entry evaluates, and `index.html` modulepreloads it. It
downloads beside the entry and parses while the startup fetches are in flight,
all under the splash.

A surface that only opens on demand, and cannot be in the first frame, gets an
**idle prefetch**. `dismissBootSplash` starts it, and the surface's opener
waits for `preload()`, so it never opens empty.

`entryChunkBudget` (`vite/entryChunkBudget.ts`) fails every single-shot build
whose entry chunk passes `chunkSizeWarningLimit`.

## Rationale

The critical path was *parse everything, then fetch*. It is now *parse the data
layer, start fetching, load the UI meanwhile*. The shell chunk's bytes are
still loaded before the first frame. What changed is that its download and
parse no longer sit in front of the network round trips the splash waits for.

That is the line between this and the `manualChunks` trick the clean-build
skill rejected. That trick moved shared code into a chunk the entry imports
statically, so the entry could not run until it arrived. The same bytes loaded
in series, and only the advisory changed.

What stays with the first frame is decided by what a cold open can draw, not by
how often a surface is used. The permission and question cards stay in the
shell chunk, because a cold open onto a waiting thread draws them at once. So
does the thread filter panel, whose open state survives a reload. An idle
prefetch starts after the splash lifts, so any of those would flash.

The guard reports rather than fails under `vite build --watch`. The dev
build-watch serves the workspace, and a failed rebuild keeps serving the old
`dist/`. So one overrun would strand every later Apply behind it.

## Consequences

- Most nightly growth is UI, and it now lands in the shell chunk. The entry
  grows only with the data layer, which is where the budget bites.
- The entry measured 488 kB after the change.
- Boot ownership hands over when the shell chunk resolves, the shape the picker
  path already had. The index.html watchdog still guards that fetch.
- A new eager import from the data layer into UI code pulls that code, and
  everything it imports, back into the entry. The guard is what catches it.
- `scrollState` stays in the entry. The store drives transcript scrolling
  through it, so it is data-layer state that happens to live in `components/`.

## Alternatives considered

- **Raise the ceiling.** Rejected by the maintainer. The number was written as
  a ceiling, and the chunk would pass the next one within weeks.
- **`manualChunks` for our own code.** Measured by the clean-build skill: the
  entry drops, first paint downloads the same bytes in two files, in series.
- **Split only the seven on-demand surfaces.** 40 kB against a 258 kB gap, and
  a loading flash on every permission prompt.
- **Split the store and actions layer.** Densely connected, and almost all of it
  runs at boot. No single action module retained more than its own bytes.
- **Fail the build under `--watch` too.** It would strand the shared
  build-watch, as above.
