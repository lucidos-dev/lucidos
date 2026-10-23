# 0044: SDK-rendered surfaces render themselves in a hostless app window; the popout stays a bare app tab

- **Status**: Accepted
- **Date**: 2026-08-05

## Context

An *app* opened through *Open in new tab* runs in a top-level window: `window.parent
=== window`, no Lucidos host around it, nothing to `postMessage` to. Four
`lucidos.ui.*` surfaces are normally rendered by the host shell and therefore have
nothing to render into there: `previewFile`, `toast`, `confirm` and `prompt`.

Each degraded differently, and all four degradations were wrong in the same way.
`previewFile` quietly delegated to `ui.navigate('file', …)`, which goes through the
engine and lands by SSE in whichever OTHER window is running the shell: the reader
clicked a citation, nothing happened in front of them, a different window navigated
its Files panel behind their back, and the promise resolved as if it had worked.
That one was fixed by making it reject (ADR-less, see
`docs/plans/2026-08-05-host-overlays-over-a-fullscreen-app.md` decision 10), which
is honest but is not a preview. `toast` writes to a console the reader never opens.
`confirm` and `prompt` fall back to `window.confirm` / `window.prompt`, which do ask
and do answer, but are unbranded and are silently disabled forever once the browser
offers its "prevent this page from creating additional dialogs" checkbox.

There were two coherent ways to fix this, and they differ on what an app *is*.

## Decision

**A host-rendered `lucidos.ui.*` surface renders itself when there is no host, and
the popout stays a bare app tab.** *Open in new tab* keeps its
`/<slug>/app/<id>/` href. The SDK grows its own file preview modal, toast, confirm
and prompt for the hostless case; with a host present, all four still `postMessage`
to it unchanged, because the host's render is strictly better where it is available.

The corollary that keeps this from becoming a second parallel implementation: the
parts both renderers need move **down into the SDK** and the host imports them from
there. The locator contract (`parseRepoPath`, `normalizeDataPath`,
`normalizeLineRange`, `resolveFileTarget`), the line-numbered row model, and the
pure dismiss-contract logic all become SDK-owned, and the CSS both surfaces paint
moves into `styles/global/shared-components.css`, which the engine already
`include_str!`s into `/api/v1/sdk-iframe.css`.

## Rationale

An app should be publishable and runnable wherever the SDK loads. Every
host-rendered surface is a dependency on the shell, so an app that needs the shell
to show a citation is not self-contained, and the fix has to remove the dependency
rather than arrange for a shell to always be nearby.

The corollary is not decoration, it is what makes the decision affordable.
Lucidos already carries one host/SDK component pair (`lucidos.ui.Select` versus the
host's Preact `Dropdown`, with `host-components.css` recording that apps get
`.lucidos-select` "instead of `.dropdown*`"), and `fileTarget.ts`'s own header
warns in writing that a second implementation of the locator contract is exactly
how the "an app must not reach a file `navigate('file', …)` would not open"
guarantee drifts apart. Moving the shared parts down means this change ends with
*fewer* duplicate definitions than it started with, not more.

## Consequences

What we keep:

- An app shows a cited file, a toast, a confirm and a prompt with no shell present.
- One locator contract, one row model, one stylesheet for anything both surfaces
  paint. The host's in-shell rendering is unchanged.
- No engine route, no new SDK transport, no Rust. The diff does touch
  `crates/lucidos-engine/src/api/sdk_iframe.css`, an engine-bundled asset, which
  obliges the matching `system-knowhow/js-sdk.md` § "Component classes" update.

What we give up:

- **The hostless preview is permanently worse than the in-shell one.** highlight.js
  core plus the 17 registered grammars is 43,964 bytes gzipped, roughly 3.7x the
  entire SDK bundle, so there is no syntax colour; and no `marked`, so no rendered
  markdown, CSV or slides. It is escaped, line-numbered source, and the modal says
  `source` out loud so it does not read as broken highlighting.
- **Every app pays the bundle**, including the overwhelming majority that never
  call any of these. A test bundles the SDK and fails over 24,000 bytes gzipped,
  which also refuses any future highlight.js import.
- **A large-file bound now exists** where none did before. It is shared with the
  host rather than SDK-only, so the same citation behaves the same in both windows.

## Alternatives considered

**A shell-carrying popout.** *Open in new tab* opens the workspace shell at a new
`#app=<id>` deep link, and the shell opens the app in pseudo-fullscreen. Every host
surface then works in that tab with no second renderer, no second locator contract,
and nothing new shipped to every app forever; the tab shows just the app, with the
existing exit-fullscreen control and Escape to reach the rest; and it fixes surfaces
not yet built. It loses only on the premise above: it arranges for a shell to be
nearby instead of removing the dependency on one, so an app published anywhere else
is no better off.

Two things found while costing it are worth keeping, because a future revival has
to answer them. It is **not free**: that tab loads the shell (roughly 176 KB gzipped
main chunk, 11 KB vendor, 36 KB CSS) and opens a second SSE connection, where today
it loads only the app. And a second shell renders **ambient host chrome unrelated to
the app**. Most of that is tolerable (a notification toast, an update banner:
transient, dismissible, and arguably wanted, since the reader is still in their
workspace), but a `NavigationRequested` is not: it would replace the app in the
content pane and drop fullscreen with it. That is narrower than it first looks,
since `NavigationRequested` is already device-scoped and thread-scoped, but the
app-iframe path is explicitly "always applies", so an app in another window could
evict the app in this one.

**Leave `previewFile` rejecting.** Ship nothing beyond the predecessor change, and
let the documented `catch { navigate('file', at) }` escalation be the answer. Honest
and free, and it keeps the cross-window navigation an explicit act by the app author
rather than a silent side effect. Rejected because a hostless app window then still
gets no glance, which is the whole point.

**Unify downward: the host's file preview modal becomes the SDK's.** The logical
endpoint of "one implementation". Rejected because an SDK renderer that has to run
inside every app cannot carry highlight.js or `marked`, so unifying downward would
make the **in-shell** preview worse for every existing user in order to serve the
hostless case. One implementation is not worth a regression on the common path.

**A lazily fetched rendering chunk.** The SDK fetches highlight.js and `marked` from
a new engine route the first time a preview opens, so the hostless preview matches
the host's while the base bundle stays near zero for the apps that never preview
anything. Rejected for now, not on principle: it needs a new engine route and a
build step to produce the chunk, which is a materially larger change for a cosmetic
gain on a glance. It is the right escalation if source-only proves too thin.

**Keep the locator contract duplicated in the SDK.** The smallest diff, and fully
self-contained. Rejected as the drift `fileTarget.ts` names explicitly: two
implementations of what a `repo:` locator addresses is how an app comes to reach,
through a preview, a file that `navigate('file', …)` would refuse.

See `docs/plans/2026-08-06-sdk-rendered-preview-for-a-popped-out-app.md` for the
implementation plan, its invariants and its verification.
