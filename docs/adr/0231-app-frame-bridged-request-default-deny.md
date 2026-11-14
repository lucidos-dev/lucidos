# 0231: An app frame reaches the engine through one bridged lucidos.request, and every route declares whether an app may reach it, default deny

- **Status**: Accepted
- **Date**: 2026-09-20

## Context

[ADR 0227](0227-app-frames-get-their-own-renderer-process.md) dropped
`allow-same-origin` from the app iframe, so each app frame gets its own renderer
process. It was bought with a measurement: a child that busy-loops for twelve
seconds starved the shell's 50ms heartbeat for all twelve, 41 ticks where 280
were due. Without the attribute the shell stayed at 280. It was also a
tightening. An app could previously read the shell's DOM, write into it, and
read a sibling app's frame. It could also lift the device id out of shared
storage.

The price is an opaque origin. The frame's own `fetch` to the engine is refused
by CORS, `EventSource` with it, and `localStorage` throws. The SDK carries the
first two over a host bridge, so `lucidos.data.*` and its neighbours work
unchanged. Everything the SDK does not name became unreachable in the shell.

That gap was recorded rather than closed. `system-knowhow/js-sdk.md` says an
endpoint with no SDK method is out of reach from an app frame, and calls a
bridged hatch a platform decision. `docs/temporary-measures.md` carries
`app-frame-escape-hatches`, an open investigation which did the measurement
work: both candidate shapes were measured viable in Chromium and WebKit against
the shipped sandbox string. This ADR is the decision that investigation waited
for, and it closes it.

**Nothing has shipped yet, and that is why this is urgent.** The isolation
commit is in no release. v0.38.2 was tagged before it landed. So every app in
every workspace meets the whole change at the next tag. That includes apps
written by people who followed none of this.

Three live examples of the gap, all from one sweep. An app reads `/env-vars` and
renders "Unavailable in the shell". Another reads `/models` to turn a model id
into a label, and shows the raw id instead. The knowhow documents
`POST /api/v1/notifications` as the way to raise a notification, with four
worked examples, and then admits none of them run inside a frame.

## Decision

**1. One generic bridged call, not a namespace per route.**
`lucidos.request(suffix, init)` travels the same host bridge every other SDK
method uses, with the same `SdkError`, the same deadline, and the same
`TimeoutError` behaviour. From a standalone app tab it goes direct, exactly as
`lucidos.proxy` already does.

**2. Every route declares whether an app may reach it, and the declaration
lives beside the routes.** A table in the engine classifies each route as
reachable from an app frame, host-only, or agent-only. Absence is denial. A test
scans the API source for every `.route(...)` and fails the build on a route the
table does not answer for.

**3. The bridge enforces that table, and stops keeping its own.** The
classification is generated into TypeScript, the way the navigate targets and
the capability table already are. `BRIDGE_ALLOWED_PREFIXES`, today's
hand-maintained prefix list in the host, is deleted in favour of it.

**4. The bridge stamps the calling app id, and the check takes it.** The host
resolves the frame to an app id it owns, and the check's signature accepts it.
It ignores the id today. Per-app grants declared in `manifest.json` are the
honest end state and are not in this change, so the plumbing lands now and the
policy later.

**5. Four groups start denied**, listed under Rationale with the reasoning
each: secrets and identity, forging the user's answer, changing the platform,
and reading outside the workspace or privately inside it.

## Rationale

**A namespace per missing route is the bug, not the fix.** It makes the SDK a
hand-maintained allowlist, and there are two of them: the SDK surface and the
bridge's prefix list. Both are correct on the day they are written. Route 191
then arrives, and nothing in the build tells its author that an app can no
longer reach it. The failure surfaces in somebody's workspace as an app that
looks broken, which is the slowest possible way to learn.

**The table has to sit where routes are written.** That is the difference
between a list somebody maintains and a question the build asks.
`crates/lucidos-engine/src/api/data_api.rs` already works this way for writable
data prefixes: the doc block carries the table, and
`every_mutable_prefix_states_what_guards_it_at_use` reads the file's own source
to prove the table covers the const. This ADR copies that pattern one file over,
including its negative control, so a long table cannot pass by looking full.

**The hatch does not make every route reachable.** It makes the question
mandatory and the answer one line. A route that should become app-reachable
later is one word in the table. No SDK namespace, no bridge entry, no docs
section.

**ADR 0156's premise expired, which is what makes an app principal possible.**
That ADR decided there is no app principal, and it said precisely why: nothing
is unforgeable downward while apps share the origin, because app A can run
`window.top.fetch` in the shell's realm. It ended by naming the one thing that
would change the answer, which was removing `allow-same-origin` or giving apps a
distinct origin. ADR 0227 did that. Inside the shell an app cannot reach the
parent realm at all. Its only channel is `postMessage`, which the host
attributes by `contentWindow` identity, and that identity cannot be forged from
inside the frame.

**The boundary binds the shell realm, and the ADR says so rather than implying a
wall.** The same app opened in its own browser tab is a top-level document on
the engine's origin. It can call any route with the user's device id, and no
classification can bind it, exactly as before this change. So the table is a
boundary where an app runs inside Lucidos, and a contract everywhere else.

### What starts denied, and why

**Secrets and identity.** `GET /credential-value`,
`POST /credential-reveal-token`, `PUT /credential-base-urls`, `GET /backup/key`,
`POST /email/send`, `/devices/register`, `/devices/hand-over`,
`/handshake-scripts/approve`, `/proxy-modules/reload`. An opaque-origin frame
can still fire a request whose answer it cannot read, and that is all
exfiltration needs. `credential-base-urls` is the subtle one: widen a
credential's allowed host, then send it there through the proxy legitimately.

Three more belong in this group and were not on the original list.
`GET /credentials` is an inventory of what the workspace holds, which is
identity even with no values in it. `GET /oauth/accounts` names the person's
connected accounts. `GET /email-account` is the user's address.

**Forging the user's answer.** The whole `/internal/` tree,
`/command-permission/consent`, `/mcp/consent`, `/mcp/auto-approve`,
`/command-checkpoint/undo`. This is the never-act-as-the-user rule in HTTP form,
and it is the worst class: the log afterwards shows a user who approved.
`POST /threads/:thread_id/answer-question` belongs here too, and is the sharpest
of them, because it answers a coding agent's question in the user's name.

**Changing the platform.** `/changes/*/apply`, `/changes/apply-all`,
`/claude-code/*`, `/engine/rebuild`, `/restart`, `/plugins/install-request`,
`/plugins/uninstall-request`, `/thread-queue/policy`. An app that can apply a
pending change can land code the user has not read.

**Outside the workspace, or private inside it.** `/browse-directories`,
`/repositories/*`, `/workspaces`, `/webhooks*`, and the reading half:
`/messages`, `/history`, `/search`, `/threads/:id/messages`,
`/session/messages`, `/memory/*`, `/voice`, `/blobs/:hash`. `POST /chat/stream`
is here as well, for the reason in the next section. A third-party plugin
quietly reading every thread and the workspace's long-term memory is the one
nobody would notice.

### The omissions that were the safety property

A generic hatch re-opens every decision the SDK made by leaving something out.
Each of these is a deliberate omission, so each names the routes it denies.

| The omission | What made it safe | Denied |
|---|---|---|
| `ui.startThread()` prefills the compose box and never submits | the user sends their own prompt, always | `POST /chat/stream`, `POST /threads`, `/threads/:id/continue`, `/threads/:id/follow-up`, `PUT /threads/:id/compose` |
| `ui.confirm` and `ui.prompt` render host dialogs | the app receives an answer, it cannot write one | the `/internal/` tree, the consent routes |
| the credential never enters the iframe, which is why `lucidos.proxy` exists | the secret stays server-side | the credential and backup-key routes |
| `oauth.getAccessToken` is the one deliberate token exit | it is scoped to one named provider, and the refresh token stays home | `/oauth/accounts`, `/oauth/complete`, `/oauth/reauthorize` |
| `threads.list` and `count` return summaries | an app sees that a thread exists, never what is in it | `/messages`, `/history`, `/threads/:id/messages`, `/threads/:id/events`, `/session/messages`, the per-event tool payload routes |
| the device id is kept from the frame (ADR 0227) | an app cannot claim to be a device | `/devices*`, `/device-presence`, `/presence-pong`, `/push/*` |
| `apps.list` and `get` return metadata | an app does not read a sibling app's source | `GET /app/:app_id/source` |
| `_capture` is driven by the host | a capture happens because the shell asked | `POST /app-capture` |
| `data.*` is scoped to the workspace tree | an app reaches the user's content, not the machine | `/browse-directories`, `/repositories/*`, `/workspaces` |

### The perimeter is smaller than the table suggests

Four holes are open today with no bridge involved. This ADR does not widen any
of them, and it must not be read as closing them.

- **`lucidos.events.query` already returns message text.** Verified against a
  live workspace: `MessageReceived` rows carry `{"text": ...}`, and the query
  endpoint filters by event type with no content restriction. So the
  confidentiality group above is defence in depth, not a wall, while that call
  stays open to any app.
- **The event stream is fanned out verbatim.** The host relays every SSE frame
  it receives to every subscribed app frame, so an app watches every thread in
  real time. This predates the isolation, since an app used to open the stream
  itself.
- **`lucidos.data.write` accepts `apps/`, `knowhow/` and `triggers/`.** Any app
  can overwrite another app's `index.html`, or drop a trigger file. ADR 0156
  decision 2 accepted this as content the user owns.
- **`lucidos.triggers.create` schedules an intent**, and an intent can do
  anything an agent can do.

## Consequences

- An app author has one documented way to reach an uncovered endpoint, and it is
  the same call in both realms.
- Adding a route now costs a classification. That is the intended friction, and
  the test states it at the moment the route is written.
- The host's prefix list is gone, so the SDK and the bridge can no longer
  disagree about what an app may call.
- The `app-frame-escape-hatches` investigation closes on its endpoint half. Its
  storage half stays open: `localStorage`, IndexedDB and CacheStorage all throw
  at an opaque origin, and no classification helps with that.
- Per-app grants stay future work, with the app id already threaded to the one
  place that would enforce them.
- ADR 0156's decision 1 is narrowed, not overturned. There is now a principal
  inside the shell. There is still none in a standalone app tab, and its egress
  decision is untouched.

## Alternatives considered

**A namespace per endpoint.** Rejected as the originating bug. It scales by
hand, it is invisible to the build, and it needs a release for each gap. It was
the plan for one day: `envVars`, `models` and notification creation. The sweep
that found the second and third cases killed it.

**A blind forwarder with no classification.** Rejected. The host attaches the
user's device id to whatever it sends, so a forwarder is a confused deputy
wearing the user's actor. The existing prefix list says as much, and a generic
call makes the point sharper rather than softer.

**A denylist written by scanning today's routes.** Rejected, and this is the
distinction the whole ADR turns on. Such a list is correct the day it is
written, and silently wrong at the next commit. The person adding a route has no
reason to think about app reach. Default deny plus a completeness test inverts
that: the new route is refused until somebody answers the question.

**Classification in the host instead of the engine.** Rejected. The host is
where the check runs, but it is not where routes are born. A table in the client
would be a second copy, drifting from the router it describes, which is the
failure this replaces.

**Waiting for per-app grants.** Rejected for the release. Grants need a manifest
field, an install-time consent panel and a stored decision. The next tag ships
the isolation either way, so the choice is a default-deny hatch now, or nothing
for every app until grants land.
