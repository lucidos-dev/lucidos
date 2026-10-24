# 0372: App storage lives in the shell's browser storage, scoped per app by the host

- **Status**: Accepted
- **Date**: 2026-10-06

## Context

ADR 0227 gave every app frame an opaque origin. Its `localStorage`,
`sessionStorage`, IndexedDB and cookies throw. Apps that wrapped `localStorage`
in a `try` kept running and silently forgot their state on every reload. The
`app-frame-escape-hatches` investigation in `docs/temporary-measures.md` held
this open.

The *app bridge* already carried a private storage transport for the SDK's own
keys. It had two defects. Any app frame could read and write every key in it.
And behind the gateway its prime came back empty. The shell's workspace
override rewrites a key on write, while the prime matched the raw prefix.

Plan: `docs/plans/2026-10-06-app-storage-for-app-frames.md`.

## Decision

Apps get `lucidos.storage.local` and `.session`, which implement the `Storage`
interface. The values live in the shell's own browser storage, keyed
`appbridge:<app id>:<sdk|app>:<key>` under the workspace namespace. The host
takes the app id from its own frame element, never from the frame. The SDK
primes the frame's values once, and apps await `lucidos.storage.ready` before
the first read.

## Rationale

- **Browser storage keeps `localStorage` semantics.** A value stays on one
  device, costs no network round trip, and survives an engine restart. That is
  what an app replacing `localStorage` expects.
- **The host is the only party that knows which app a frame runs.** It set the
  frame's `src`. Anything in a message is the app's own claim.
- **Two key spaces per app.** An app's `clear()` must not wipe the SDK's scroll
  memory, and an app key must not collide with an SDK key.
- **One shared definition of the limits.** The SDK checks the quota and value
  cap so `setItem` can throw `QuotaExceededError` synchronously, as
  `localStorage` does. The host re-checks with the same constants, because it
  does not trust the frame.
- **An async prime is the honest limit of this design.** The shell cannot put
  anything into an opaque frame synchronously. Apps already await every bridged
  engine call, so one more `await` at startup fits.

## Consequences

- An app migrates by renaming its calls and awaiting `ready` once.
- A read before `ready` sees nothing stored. The SDK warns once in the console.
- A write the host refuses after the fact is rolled back in the mirror,
  reported to the app's `onError` handlers, and toasted to the user.
- Two open frames of one app keep separate mirrors, and no `storage` event
  fires between them.
- Deleting an app clears its keys on every device with the shell open when
  the `AppDeleted` event arrives. A device that was closed keeps its copy, and
  an app later installed there under the same id inherits it.
- The SDK's own scroll position is written without waiting for an answer.
  The host enforces a small cap on that space and logs a refusal to the
  console. A full store also fails the app's own writes, which toast.
- IndexedDB, `caches` and cookies get no replacement. `lucidos.data` holds large
  or shared state.

## Alternatives considered

- **An engine-side store, served at first paint like `sdk-prefs.js`.** It would
  make the first read synchronous with no `ready`. It loses because it changes
  what the store is: state would follow the user across devices, every
  `setItem` would become a network write, and it needs a migration and a new
  route. Session storage would still have nowhere to live. Offered to the
  maintainer as the fork at plan approval, and declined.
- **A `window.localStorage` shim** over the same mirror. Measured to install in
  both engines. Rejected because an unchanged app reads at startup, before the
  prime answers, so the shim would still forget its state while looking like it
  works. It also replaces a global that third-party libraries probe.
- **Delivering the snapshot in the frame's `name` attribute**, which the frame
  can read synchronously. Rejected as untested across WebKit and the packaged
  client, and because it couples storage to how the shell names frames.
