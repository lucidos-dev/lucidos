# 0227: An app frame gets its own renderer process, so it cannot freeze the shell

- **Status**: Accepted
- **Date**: 2026-09-19

## Context

A user reported Chrome's **Page Unresponsive** dialog. It named the Lucidos tab
rather than the app that caused it, and a reload landed back on the same dead
page. The content pane held an app streaming a remote browser.

`AppUiInline.tsx` mounted an app with
`sandbox="allow-scripts allow-same-origin …"` against the engine's own origin.
Same origin is the same renderer process, so the app and the shell shared one
main thread.

Measured here, against a child that busy-loops for 12 seconds while the host
ticks a 50 ms heartbeat:

| App frame | Host ticks (280 due) | Worst gap |
|---|---|---|
| Same origin, the sandbox as it was | 41 | 12,005 ms |
| Same origin, `allow-same-origin` dropped | 280 | 51 ms |
| Separate port, `Origin-Agent-Cluster: ?1` | 280 | 52 ms |
| Separate hostname (cross-site) | 280 | 51 ms |

Under `--site-per-process`, the desktop Chrome default. No host-side code can
run while the app owns the thread, so nothing except a separate process helps.

The same attribute was also the app's reach into the shell. The same probe, run
against both sandbox strings:

| An app frame can… | Before | After |
|---|---|---|
| Read the host DOM (`parent.document`) | yes | BLOCKED |
| Read host storage, including the device id | yes | BLOCKED |
| Write into the host DOM | yes | BLOCKED |
| Read a sibling app's frame | yes | BLOCKED |

An app is not always code the user wrote: the agent authors them and plugins
ship them. Each ran with the shell's authority and could call the engine as the
user with the device id it read out of shared storage.

## Decision

Drop `allow-same-origin` from the app iframe. What the frame then cannot do for
itself, the host does for it over `postMessage`: engine calls, the event stream,
and storage. The host's three reach-ins (capture, app switch, fragment delivery)
become requests the frame answers.

`components/apps/appFrameSandbox.ts` holds the attribute and derives
`APP_FRAME_ISOLATED` from it, so the frame, the navigation and the capture
cannot disagree about which realm they are in.

## Rationale

The cheapest of the three measured routes, and the only one needing no second
port, hostname, CORS policy or auth story. It is also the strongest: the other
two leave the app a real origin, where this leaves it none.

The bridge forwards a named set of `/api/v1` sub-trees rather than any path,
because the host attaches the user's device id to what it sends. A blind
forwarder would be a confused deputy wearing the user's actor. It strips
`x-lucidos-*` from what an app supplies before stamping the real id, so an app
cannot name a device it is not. A source scan pins the list against the SDK's
own surface, and caught `/oauth` missing before it shipped dead.

One residual channel survives. A sandboxed frame can still fire no-CORS
subresource requests, cannot read their responses, and cannot set
`x-lucidos-device-id`, so such a request arrives unattributed.

## What the browser suite found, and what it cost

Three things only a real browser said, each a correction to the plan.

**The engine's own gate refused the frame.** `api/browser_origin.rs` admits
`Sec-Fetch-Site: same-origin` or `none`, and an opaque origin sends
`cross-site`. So `/api/v1/sdk.js` answered 403 and `lucidos` was undefined in
every app. Its module doc said an app iframe passes deliberately, which was true
only while the frame shared the engine's origin. `is_public_app_asset` now
exempts the handful of `/api/v1` assets an app loads as tags on its own
document. Nothing that reads or writes the workspace is exempt, and `/app/*` and
`/data/*` are nested outside the gate and never saw it.

**Navigation cannot ask the app, because the SDK is opt-in.** The bridge carried
the app switch, and an app that loads no `sdk.js` answers nothing. `AppUiInline`
keys the frame on the document instead, so a switch remounts the element. An
iframe's first load is a history replace, the property the imperative
`location.replace` existed to keep, and a remount needs nothing from the app.
`navigateAppIframe` is gone.

**A cross-origin `<a download>` is ignored by the browser**, so a click on one
of an app's own files would navigate the frame to it. `sdk.js` rewrites the
click to ask for the file as an attachment, and `serve_app_file` answers
`?download=1` with `Content-Disposition`. This one DOES need the SDK: a frame
that loads none keeps `blob:` and `data:` downloads and loses that link shape.

## Follow-up: the exemption needed a second half

`is_public_app_asset` landed on the engine alone. The browser suite runs direct
to an engine, where no credential is asked for, so nothing caught what happens
behind the gateway.

A gateway fronts every packaged install and every remote-access URL, and it asks
each request for the device credential. That travels in a `SameSite=Lax` cookie.
An opaque origin has a null site-for-cookies, so the browser sends no cookie
with any subresource of the frame's document. `auth_api::enforce` answered 401 a
hop before the engine, and every app in every workspace rendered unstyled with
no `lucidos` global.

`lucidos-gateway`'s own `auth_api::is_public_app_asset` is the matching half,
and a source scan pins the two lists together. See
`docs/plans/2026-09-20-the-gateway-half-of-the-app-asset-exemption.md`.

## Consequences

- An app that reaches the engine through `lucidos.*` is unchanged. Every call
  keeps its signature.
- An app that calls `fetch('/api/v1/…')` or `localStorage` directly loses it.
  That is the accepted cost, and it is loud rather than silent.
- Popups, OAuth and fullscreen are untouched.
- Subresources hold DIRECT to an engine, and behind a gateway only the exempt
  `/api/v1` assets do. An app's own `<script>`, `<link>`, `<img>` and
  `lucidos.data.url(path)` are subresources of an opaque-origin document, so
  they carry no device credential and the gateway refuses them. Measured in a
  real browser, not inferred. That is the residual this decision leaves open,
  and it is registered in `docs/temporary-measures.md`.
- The `<a download>` rewrite above goes the same way behind a gateway. It asks
  for `/<slug>/app/<id>/<file>?download=1`, which is one of those refused
  paths, so the same row covers it.
- The first-paint appearance script had to move first. It read theme, font and
  UI scale out of the shell's storage, which only `allow-same-origin` made
  visible. It is parser-blocking, so nothing async can replace it. The engine
  now resolves those values and serves them inside the script. Every isolation
  route needed that, which is why it landed on its own.
- One relayed event-stream connection replaces one `EventSource` per open app.
- A standalone app tab is a top-level document and keeps every direct path. The
  SDK branches on `window.origin === 'null'`.

## Alternatives considered

**A separate port with `Origin-Agent-Cluster: ?1`.** Measured equal on
isolation, and it keeps the app a real origin, so `localStorage` and a direct
`fetch` survive under CORS. Rejected on cost: engine and gateway port plumbing,
a CORS policy, and a device-auth story for an origin whose storage starts empty.
It needed the appearance change anyway, for that same empty storage.

**A separate hostname.** Measured equal again, and everything the port route
costs plus DNS and certificates.

**Keeping the frame same-origin and making the shell resilient.** Not possible.
One process is one main thread, and no host-side code runs while the app holds
it. This is what the first row of the table settles.

**Withholding a restored app after it hung the tab.** An escape hatch rather
than a fix: it treats the symptom, costs the restore the user reached for, and
leaves the next app with the same power. Rejected by the maintainer in favour of
isolation.

**The app-side fix alone.** The reported app was rewritten to stream raw JPEG
and decode off-thread, which is right for that app. It fixes one app, and the
next one inherits the same power over the shell.

**Testing the isolation by asserting the sandbox string.** A string assertion
passes whatever the browser then does with it. The browser spec blocks a real
app's main thread and asserts the shell's own timer keeps ticking, which is the
property rather than its spelling.
