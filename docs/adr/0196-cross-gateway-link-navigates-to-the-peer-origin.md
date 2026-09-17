# 0196: A cross-gateway thread link navigates to the peer gateway's own origin, and the peer's scheme is probed rather than assumed

- **Status**: Accepted
- **Date**: 2026-09-16

## Context

A thread reference is workspace-qualified, `[title](thread:<workspace>/<uuid>)`.
`lucidos spawn-thread` prints that shape for an agent to paste, and the copy
action writes it. Clicking one resolves the workspace name against the gateway
serving the page.

A machine can carry several install vehicles at once, each with its own gateway,
port, registry and Postgres ([ADR 0189](0189-coexisting-installs-are-told-apart-by-port.md)).
Coexistence is a supported setup rather than a trap, and the ordinary developer
one: the packaged app on 5252 beside a source checkout on 5251.

The resolver looked at one gateway only. So a link into a workspace the OTHER
install serves ended at `Workspace 'dev' is not available`, which is a lie: the
workspace exists, it is running, and it is one port away. The user reported it
by clicking such a link from a workspace on the packaged install.

## Decision

**The link navigates to the peer gateway's own origin, and nothing is
proxied.** The tab lands on `<scheme>://<host>:<peer port>/<slug>/#thread=<uuid>`.
The peer gateway authenticates the browser itself. No workspace data, no
credential and no session crosses a gateway boundary.

**The gateway locates a peer by NAME, and never enumerates.**
`GET /~/api/v1/control/workspace-location?name=…` reads the install inventory,
skips this install, and reads each peer's `config/workspaces.json` from disk. It
answers with one workspace or a 404.

**The peer's scheme is measured.** The route probes the matched install's
gateway on `/~/api/v1/health` and reports whichever scheme answered. That is
also the liveness check.

**A peer URL is composed only when this page reached its own gateway on its own
port.** The client compares `location.port` against the stamped `GATEWAY_PORT`.
Behind a proxy it declines and says where the workspace lives instead.

## Rationale

**Proxying would widen a pairing, which ADR 0132 refused in as many words.**
That decision split auth state per gateway, so a device pairs to a gateway
rather than to a machine. It rejected keeping one shared store precisely because
it "widens a code minted on a dev checkout into authority over the packaged
install's workspaces". A proxy hop would do that widening silently, for every
link. Navigation keeps the boundary where 0132 put it: an unpaired browser gets
the peer's own pairing screen, which
[ADR 0094](0094-gateway-authenticates-every-network-caller.md) already treats as
the honest answer to an unauthenticated navigation.

**The scheme cannot be assumed, and this is measured rather than argued.** On
the machine that reported the bug, the packaged gateway serves plain http and
the dev checkout serves https from mkcert certs. TLS comes from
`LUCIDOS_TLS_CERT` and `LUCIDOS_TLS_KEY` at launch, and is recorded nowhere on
disk. Swapping only the port composes a URL that never loads, and the failure
reads as a broken link rather than a scheme mismatch.

**Probing beats publishing, because the peer that matters is old.** The
alternative was each gateway writing its own port and scheme to disk at boot, in
the shape of [ADR 0136](0136-running-engine-publishes-its-ports-file.md). It
needs BOTH gateways to carry the new code, and a packaged one only moves on a
new DMG. A probe needs nothing from the peer, so this works in both directions
the day it lands. That is
[ADR 0105](0105-engine-can-be-newer-than-the-gateway.md)'s rule: degrade on the
older side, never fail closed. The same reasoning put the install scan itself in
a shared crate under ADR 0189.

**Loopback is always the right address to probe.** Every `BindChoice` reaches
it. `Loopback` is it, `All` contains it, and an explicit `Address` binds loopback
beside itself. So the probe needs no view of the machine's bind.

**A lookup discloses less than a listing, and costs nothing extra.** The caller
already holds the name, having read it out of a link. Answering where that one
workspace lives tells them nothing about the peer's others. It also keeps the
route out of the picker's business: the picker offers start, stop, rename and
delete, and this gateway can do none of them to another install's workspace.

**The port guard is what keeps the feature from lying in the other direction.**
The URL is composed by swapping a port. Under `tailscale serve` or an ssh
forward, the page arrives on a port mapped to one gateway, so the swap addresses
nothing. Naming the install and its port beats opening a tab that cannot load.

## Consequences

- A cross-gateway link works between any two installs on one machine, with no
  change to the peer. The install a user is stuck behind is the old one, so this
  is the property that matters most.
- **The browser may have to pair with the second gateway.** That is ADR 0132's
  shipped behaviour, and one browser can hold a pairing to every gateway on a
  hostname at once. The landing is the peer's pairing screen, not an error.
- **The cross-workspace title popover still shows a short id for a peer
  thread.** Reading a title over a gateway boundary needs CORS plus credentials,
  which is the widening this ADR refuses. The link's own text carries the title,
  so the cost lands on the popover alone.
- The install inventory gains a second consumer. It was Settings, the client
  preflight and the uninstallers; a click on a thread link now walks the same
  directories. It is hand-opened rather than polled, exactly as ADR 0189
  required of the inventory route.
- **Nothing across the boundary is started.** A stopped peer gateway is named,
  never launched: its launch agent belongs to another install. A stopped
  workspace on a reachable peer needs nothing, since that gateway lazy-starts it
  on the proxy hit.
- **An older gateway answers the new route with the picker shell, not a 404.**
  An unmatched `/~/…` path falls through to the SPA fallback, so the reply is
  200 and HTML. The client reads a body it cannot parse as "cannot say", which
  is the pre-change behaviour. This is not hypothetical. The frontend is served
  by the engine, so a current frontend behind an old gateway is the ordinary
  shape of the machine this exists for.
- The gateway gained `rcgen` as a dev dependency, for a self-signed cert the
  https stand-in mints per run. A checked-in cert would expire and fail a test
  years from now.

## Alternatives considered

**Proxy the peer's workspace through this gateway** (`/~peer/<port>/<slug>/…`).
It would work from any access context and need no second pairing, which is
genuinely attractive. Rejected: it widens a pairing minted here into authority
over another install's workspaces. That is what ADR 0132 decided against, and a
proxy would do it for every link rather than as a deliberate act.

**Each gateway publishes its port and scheme to disk at boot.** Cheaper at
lookup time, with no probe and no permissive TLS client. Rejected because it
needs the peer to carry the new code, and the peer this exists for is the one
that will not for months.

**Put peer workspaces in the picker and the in-app switcher.** The natural next
want, and rejected for this change. Both surfaces offer management actions this
gateway cannot perform on a peer's workspace. A row that can only be opened
reads as a broken row. A later change may add an open-only shape.

**Let the client probe the peer itself.** No new route, no backend. Rejected as
not possible rather than unattractive: a cross-origin response is opaque, so the
browser cannot read the peer's health body, and it cannot read another install's
registry at all.

**Make the copy action write an absolute URL** instead of `thread:<ws>/<uuid>`.
It would work today with no code, since
[ADR 0038](0038-a-chat-link-never-leaves-the-workspace.md) passes an `https:`
href to the browser. Rejected on two counts. It bakes one host into a reference
meant to be pasted anywhere, so a link copied on the desktop breaks on the
phone. And it loses in-app routing for the same-workspace case.

**Encode the peer's scheme in the link.** Same flaw one level down, plus it
would go stale the first time a checkout gains or loses its certs.
