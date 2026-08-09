# 0055: The frontend preview is a separate origin, never the workspace's serving path

- **Status**: Accepted
- **Date**: 2026-08-08

## Context

A design conversation about the app header hit the wall every design
conversation about this app hits: a CSS custom property can be retuned live
through the `style_overrides` preference, but a `.tsx` change cannot. The only
way to see one is Apply, which merges the coding agent's branch into `main`. So
the loop stops at the first structural change, and the user asked for a "fast
lane" instead: a preview checkout of the TypeScript with Vite hot reload.

Two prior decisions make that non-obvious to reopen.

**ADR 0014 §5** retired Vite from the serving path: "Vite becomes build-only;
the dev reverse-proxy and HMR are removed", one serving path in dev and
packaged, the engine serving `dist/` directly. The `dev_proxy.rs` it deleted was
the root cause of a class of gzip and HTML-rewrite bugs.

**ADR 0021** forbids a long-lived stack rooted in a coding-agent worktree, after
an entire running stack was found executing out of an orphaned, pruned worktree
for hours. The user-visible symptom was silence: every frontend Apply landed
correctly on `main` and the served bundle never moved.

## Decision

The **frontend preview** is a Vite dev server the engine spawns and supervises
inside a coding-agent worktree, listening on **its own port**. It is never the
workspace's serving path: it does not touch `LUCIDOS_STATIC_DIR`, does not swap
the served-frontend handle, and the workspace URL keeps serving `dist/`
unchanged whether a preview is running or not. It is refused on a packaged
build, and there is one slot per workspace.

## Rationale

**Neither prior decision is reversed, because neither is about this.**
ADR 0014 §5 is about what the workspace's own URL serves, and that is still one
path in dev and packaged. ADR 0021 is about a stack *silently* pinned to a
worktree, where the failure is invisible; a preview the user started, on a port
they opened deliberately, with a Stop button, fails loudly or not at all.

**The engine has to own the process.** A coding-agent session's whole process
group is killed when its turn ends, so a `vite` the agent starts dies with the
message and the next turn starts it over. Supervision is the entire feature, and
it is why this is engine code rather than a script.

**A separate port avoids a rewrite of the base-path contract.** The bundle
derives `BASE_PATH` from the `<base href>` the engine stamps, and its workspace
slug from that (`utils/basePath.ts`). A nested `/<slug>/preview/<thread>/`
prefix fails the slug shape, so `baseContextIsValid()` bounces the app to the
picker. Making a prefix work means changing base-path parsing, the API prefix,
the service-worker scope, the gateway's routing table and the engine's shell
stamping: five surfaces, each of which the whole product boots through. On its
own port the bundle takes the `BASE_PATH === ''` branch, which is the
already-supported legacy direct-engine mode, and needs none of them.

**Vite proxies the engine-owned prefixes back to the engine**, so the preview
page is same-origin with its own API and no CORS is needed. That block is gated
on the env var the engine sets, so a manual `npm run dev` is unchanged.

## Consequences

- **The preview has its own `localStorage`**, being its own origin, so it would
  mint a new device id and lose this device's scoped preferences. The preview
  link carries `?device-id=`, which a dev-server bundle adopts and strips
  (`utils/deviceIdSeed.ts`). Everything else in that bucket (last focused
  thread, scroll positions) starts fresh, which is accepted.
- **No service worker on the preview.** A dev server emits unhashed module URLs,
  so a worker caching them would serve stale modules after a hot update, and the
  `sw.js` Vite serves carries an unstamped `__LUCIDOS_BUILD_ID__`. Registration
  is gated on `isDevServerBundle()`, and push therefore cannot be enabled there.
- **A second self-signed cert prompt** on a device that has not accepted one for
  that port. The preview reuses the engine's own `LUCIDOS_TLS_CERT` / `_KEY`.
- **A node process outlives the agent turn, by design.** It stops on an explicit
  stop, on its worktree disappearing, and on engine restart. Deliberately no
  lifetime timer: one that fires while the user is looking at the preview is
  worse than a lingering process, and in dev an engine restart reaps it often.
- **An orphan is possible and is reaped.** A SIGKILLed engine leaves the child
  alive, so the running preview is recorded in
  `<workspace>/.lucidos/frontend-preview.json` and the next boot kills the pid
  only if its command line still names both `vite` and that worktree
  (the ADR 0025 shape: an unreadable command line is never a yes).

## Alternatives considered

**A path prefix under the workspace** (`/<slug>/preview/<thread>/`), same origin
so `localStorage`, the accepted cert and the device id all come for free. Lost
on cost: the five-surface base-path rewrite above, plus a gateway route, plus
HMR websocket upgrade through the gateway. It remains the better end state if
previews ever become a shipped feature rather than a dev affordance.

**Repointing the engine's served-frontend handle at a build of the worktree.**
The mechanism already exists (`engine::frontend_refresh` swaps it for a
frontend-only Apply), so this looked nearly free. Rejected as exactly the ADR
0021 failure with a friendlier face: the whole workspace would serve a WIP
build, a broken one would take the real app down, and the pin would be invisible
in the UI it broke.

**A per-thread `vite build` served as a static preview dir.** No process to
supervise, but no hot reload either, and the user asked for hot reload by name.
It also still needs a place to serve from, so it inherits the base-path problem
without solving the thing it was traded for.

**A parallel component lab** rendering the header out of real components, and
**a hand-written HTML mockup** (the shape used for the July accent retheme).
Both rejected by the user in the originating thread: "prototype should be the
real thing i guess, otherwise it would be quite stupid." A copy is never what
ships.
