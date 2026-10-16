# 0069: The client bundle carries its own build id, never the engine's CalVer VERSION

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

The System page shows a "Client" version, and the refresh badge has to decide
whether the loaded code is stale. Both need one honest answer to "which build
produced the code running right now?".

The engine has a CalVer `VERSION` that bumps on every Apply, including an
engine-only one. A `virtual:engine-version` Vite plugin used to bake that string
into the client bundle so the System page could show it.

## Decision

The engine's `VERSION` is not baked into the client bundle. The client's
identity is `CLIENT_BUILD_ID`, supplied by the `virtual:build-id` virtual module
and stamped by the `lucidos-sw-stamp` plugin with the same id it writes into
`sw.js`.

## Rationale

A baked engine version is a lie by construction. It freezes at whatever
`VERSION` was when the bundle was last built, while the running engine's own
`VERSION` keeps bumping on every engine-only Apply. That puts two numbers on one
page that can disagree, with nothing the user can do: no reload changes a baked
string.

`CLIENT_BUILD_ID` is derived from the emitted asset filenames, which embed
content hashes, so it is deterministic. A no-op rebuild yields the same id and
reports no update.

It is also the id `sw.js` carries. The client therefore compares the build that
produced the executing code against the served `sw.js`. The alternative
comparand, the controlling service worker, can run ahead of the loaded page
after a claim without reload.

## Consequences

The System page shows the client's own build id, and the refresh badge compares
against it. There is no client-side view of the engine's `VERSION`, which the
engine reports over its own API when it is genuinely wanted.

## Alternatives considered

**Keep the baked engine version and re-bake it on every bump.** Rejected. Every
engine-only change would then produce a byte-different bundle, so a new `sw.js`
BUILD_ID, so an update toast whose entire payload is a version string. Today an
engine-only Switch correctly surfaces nothing (`store/actions/connection.ts`).

**Keep it and let it drift.** Rejected, and this is what shipped first. The
`addWatchFile` meant to re-bake the value went inert when the `--built` dev mode
replaced `vite build --watch` with `dev-build-watch.mjs`'s one-shot builds,
which do not watch `VERSION`. A row that is silently wrong is worse than no row.
