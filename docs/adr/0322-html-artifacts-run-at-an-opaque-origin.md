# 0322: An HTML artifact runs at an opaque origin, in the preview and when served

- **Status**: Accepted
- **Date**: 2026-09-29

## Context

The nightly `/harden-project` sweep rated this critical. An HTML artifact ran
with the shell's full authority, reachable two ways.

1. **The preview.** `FilePreviewInline` rendered an `.html` data file in
   `<iframe srcDoc>` with no `sandbox`. An `about:srcdoc` document inherits the
   embedding page's origin, and the shell carries no CSP. So the artifact's
   script could read `parent.document` and the device id in `localStorage`. It
   could call every host-only route as the user, a credential reveal included.
2. **Direct serving.** `/data/*`, `GET /api/v1/data/*path` and
   `/app/:id/artifacts/*path` served `.html` and `.svg` inline on the
   workspace origin. Opening such a URL ran the file as the shell.

An artifact is untrusted content. It comes from uploads and from pages and mail
an agent fetched. Isolated apps write it too, and an app can then open the file
in the host preview. So this also undid the app isolation of
[ADR 0227](0227-app-frames-get-their-own-renderer-process.md).

The shell reached into the preview's `contentDocument` for three things: link
routing (`previewIframeLinks.ts`), shell shortcuts (`previewIframeShortcuts.ts`)
and fragment scrolling. ADR 0217 and `docs/known-gaps.md` described the preview
as same-origin because those bridges relied on it.

The plan is `docs/plans/2026-09-29-html-artifact-preview-runs-at-an-opaque-origin.md`.
It offered a thorough and a narrow variant, and the maintainer chose the
thorough one.

## Decision

1. **The preview frame is sandboxed without `allow-same-origin`.** Its tokens
   are `allow-scripts allow-forms allow-modals allow-popups allow-downloads`.
   An artifact's own scripts keep running, at an opaque origin.
2. **The bridges cross a validated `postMessage` channel.** The host stamps a
   small script into the srcdoc, as the first thing it parses. It cancels three
   things and posts each up: a click on a link the host routes, a download of a
   sibling file, and a chord bound to a shell shortcut. The host sends fragment
   scrolls and pass renewals down. Every routing decision stays on the host.
3. **The host trusts a message on four checks, all required.** The sender is
   that frame's own window. The origin is `"null"`. The nonce matches this
   render. The body has the exact shape of its kind.
4. **Only a real event reaches the host, under a nonce the artifact cannot
   read.** The bridge removes its own script element and keeps the nonce in its
   closure. It posts only for a trusted event, as an object literal no
   prototype setter can intercept. The host also acts on a link or download
   only while it holds user activation, as a second lock. A local file opens
   only after the user confirms.
5. **The preview carries a narrow asset pass.** A frame capability
   ([ADR 0238](0238-app-frame-carries-a-capability-to-its-own-files.md)) is
   minted for a reserved subject, `artifact..preview`, and spliced into the
   `<base href>` the preview already stamps. It reaches `/data/artifacts/*`
   only. The host mints it from `GET /api/v1/artifact-preview-capability`, and
   renews it at half-life with the app frames' passes. The "Open in new tab"
   link of an HTML artifact carries it too.
6. **Served documents carry `Content-Security-Policy: sandbox`**, with the same
   tokens, on all three data mounts. That covers HTML and every XML type, SVG
   included, and a `304` for such a path. Every other type keeps its exact
   headers.

## Rationale

**The origin is the boundary, so the fix removes the origin.** Anything short
of that leaves the artifact's script inside the shell's origin, where no check
can tell it from the shell. A CSP on the shell would be a partial answer and a
large project: the shell has inline styles and dynamic imports everywhere.

**Scripts keep running because artifacts are interactive.** Reports carry
sortable tables and charts. The narrow variant would have kept the bridges
untouched and stopped every script. The maintainer chose to pay for the bridge
rewrite instead.

**The frame decides what to cancel; the host decides what it means.** A
browser default can only be cancelled synchronously, inside the event. The host
is a message away. So the host stamps two lists into the script: the URL schemes
it routes, and the current shortcut bindings. Both come from the constants the
host routes with, and a unit test pins the scheme list to the router. That is
also why a matched chord still cancels the browser default, which the app
frames' SDK forwarder cannot do.

**A download is claimed because the browser would navigate instead.** A
sibling file sits on the shell's origin, which is foreign to the opaque frame.
A browser ignores `download` on a cross-origin link and navigates the frame to
the file, which replaces the report. The host downloads from its own origin,
where `download` holds.

**Origin `"null"` is checked, never trusted.** Every opaque frame reports it,
an app frame included. So the source-window check is what identifies the
sender. The origin check is a tripwire instead: if the sandbox ever went away,
the origin would be the shell's, and the bridge would stop rather than keep
trusting.

**The nonce is per render.** It also separates this document from the one the
frame showed before a re-render, whose late messages are dropped.

**The nonce is the gate, so the artifact must not hold it.** The artifact's
script shares the frame with the bridge. If it can read the nonce, it can post
a link on load. A fetched page could then open files, threads, apps and URLs
unasked. On the desktop app it could hand a local path to the OS opener, which
launches whatever the path names.

User activation alone does not stop that. The click that opens an artifact
leaves the shell activated for a few seconds, and a script that posts on load
lands inside them. The browser e2e forger proved it. So the bridge hides the
nonce and ignores untrusted events, and activation stays only as a second lock.

**A local file asks first, even on a real click.** A hostile report can cover
itself with one invisible link, so any click inside it is a click on that link.
The OS opener is the one destination where that costs more than a detour.

**The pass exists because an opaque origin gets no cookie.** A frame at an
opaque origin has a null site-for-cookies. The browser withholds the
`SameSite=Lax` device credential from its subresources. Behind a gateway, every
relative image and stylesheet in a report would 401, on every packaged install
and every remote-access URL. ADR 0238 solved exactly this for app frames.

**The pass is narrow because the content is hostile.** An artifact can read
its own base and send the pass anywhere. An app's pass reaches all of `data/`,
which an app can already read over the bridge. A report has no such grant. So
its pass reaches `artifacts/`, the tree its siblings live in, and nothing that
holds configuration, scripts or signers.

**The reserved subject cannot collide with an app.** `apps::is_valid_id`
refuses `..`, and the subject contains it. So no real app can be minted a pass
that the gateway would read as the narrower preview grant.

**The engine half closes the preview's back door.** Without it, a script in
the sandboxed preview could ask the host to open a same-origin `/data/…html`
URL in a new tab. That tab would run at the workspace origin with full
authority. With the header, it runs opaque too.

## Consequences

- A previewed artifact cannot read the shell, cannot use storage, and cannot
  call `/api/v1`. The engine refuses its requests as cross-site
  (`api/browser_origin.rs`), and CORS hides the rest.
- Link routing, fragment scrolls, shell shortcuts and the zoom shortcuts work
  as before, on desktop and phone. The zoom itself needs no bridge: it is
  stamped into the document text (ADR 0217).
- **A real click can still land on a link the artifact disguised.** A hostile
  report can cover itself with one invisible link. The worst it reaches is a
  file, thread, app or URL opening, and a local file still asks first.
  Likewise a real key press can run a shell shortcut while focus is inside it.
- **An artifact cannot `fetch()` a sibling file.** The request is cross-origin,
  and ADR 0289 deliberately grants a CORS read to no `fetch()`. A report must
  carry its data inline, or be an app. This worked while the preview shared the
  shell's origin, and the knowhow now says so.
- **The pass can leave the device.** A hostile artifact can send it to its own
  server. Anyone who can reach the gateway can then read `artifacts/` for up to
  an hour, and longer while the preview stays open and renewals land.
- **An HTML file previewed from outside `artifacts/` gets no pass.** Behind a
  gateway its relative assets 401. The document itself still renders.
- **An HTML artifact opened as its own tab runs opaque, like the preview.**
  The header's "Open in new tab" link carries the pass for its hour. A bare
  `/data/…html` URL typed or linked elsewhere loses its relative assets behind
  a gateway, since the tab's subresources get no cookie either.
- **A pass renewal is posted to the frame whatever document it shows.** An
  artifact that navigates its own frame elsewhere receives later renewals. It
  could have leaked the first pass anyway, so this widens nothing.
- A download of a sibling file works in a browser. The desktop app drops a
  download from its main window, as it always has.
- The PDF preview is untouched and keeps its same-origin shortcut bridge. The
  repo file preview already renders HTML as source.
- `/app/:id/*path`, an app's own bundle, gets no CSP. An app frame is isolated
  by its iframe sandbox (ADR 0227), and a standalone app tab needs its origin.

## Alternatives considered

**The narrow variant: `allow-same-origin` without `allow-scripts`.** The
bridges would have kept working untouched. Every script in every previewed
artifact would have stopped. The maintainer rejected it in favour of the
thorough variant.

**A separate origin for artifacts.** A second port or hostname, with the
gateway routing to it. It needs a certificate story, port plumbing, and a
device-auth design for an origin whose storage starts empty. ADR 0227 priced
the same idea for apps and turned it down. The CSP sandbox gives the same
isolation with none of that.

**Refusing `local-file` from the preview outright.** A report linking a local
PDF is legitimate. One confirmation keeps it working and puts the user in the
decision.

**User activation as the only gate.** It passed the forger in every engine: the
click that opened the artifact was still active when its script posted.

**Reusing the app pass for the preview.** One pass reaching all of `data/`
would have been less code. It would also have handed a hostile document a
leakable token for configuration, scripts and signers. The reserved subject
costs one `admits` arm.

**A pass scoped to the artifact's own folder.** The token's subject is one
URL-safe segment, so a folder would need an encoding and a length budget. A
report at `artifacts/` itself would get the whole tree anyway.

**Granting a CORS read for `fetch()` behind a gateway.** Only a request
carrying a pass could use it, so it would expose nothing new. It reverses ADR
0289's "a `fetch()` never gets the grant", which deserves its own decision.

**Leaving the served-file half for later.** The preview fix alone was the
smaller diff. It left the new-tab route to full authority described above, so
the preview fix would not have held.
