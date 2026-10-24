# 0163: The gateway answers a WebSocket handshake's same-origin question

- **Status**: Accepted
- **Date**: 2026-08-29

## Context

ADR 0151 gave the gateway a WebSocket upgrade path and left the same-origin
question to the engine's existing gate, `api::browser_origin`. Its last
consequence bullet says every current browser passes that gate, on the grounds
that WebKit has sent `Sec-Fetch-Site` on a handshake since 2022.

That is wrong about the browsers that matter. **Chromium and Gecko send no fetch
metadata on a WebSocket handshake at all.** The header set is built by "establish
a WebSocket connection", not by the fetch pipeline. So the gate's authoritative
arm never runs for a socket, and every handshake falls through to the legacy
`Origin == Host` comparison.

Behind the gateway that comparison cannot succeed. `proxy_upgrade` sets `Host` to
the internal engine authority, and forwards the browser's `Origin` verbatim. The
first real call from Chrome was refused for exactly this:

```text
[browser_origin] refused a cross-origin browser request to /voice
  (sec-fetch-site: None, origin: Some("https://localhost:5251"))
GET /api/v1/voice → 403
```

`/ws-echo` was refused with it, so the client's own probe agreed there was no
route, and the reader was told the engine might be down.

The gate's own doc already calls a no-fetch-metadata browser behind the gateway
an accepted limitation, because the population is old and shrinking. For a
handshake that population is every Chrome and Firefox user.

## Decision

**The gateway answers the same-origin question for an upgrade it proxies, and
consumes the header it judged.**

- A handshake whose `Origin` does not match the gateway's own request `Host` is
  refused there with a logged 403, before the engine is dialled.
- A handshake with no `Origin` is not from a browser and passes, exactly as the
  engine's gate leaves such a caller to the bind topology.
- `origin` joins `gateway_owns_header` on the **upgrade path only**. The engine
  then sees a hop carrying none, which is what a spliced handover from a local
  process is.
- The HTTP path is untouched, and the engine's gate is unchanged.

## Rationale

The gateway is the only hop that still holds the client's own authority. The
engine cannot answer the question behind it and never could: its `Host` is the
upstream address, and no `x-forwarded-host` is injected.

`Origin` is trustworthy here on the same terms `Sec-Fetch-Site` is trustworthy
elsewhere. The `WebSocket` constructor takes no headers at all, so page script
cannot touch it. The only missing input was ever what our own authority is, and
this hop has it.

The check is new protection rather than a relocation. Nothing checked a
handshake's origin behind the gateway before, and pairing does not cover it: a
page on another port of this machine is same-site, so its cookie rides along.
That is the attack `browser_origin` was built for, and the socket path had no
answer to it.

Consuming the header is the move `hook_socket::forwarded_to_engine` already
makes, for the same reason and with a test that names it. A delivery the socket
has authorized must not then be refused by a gate with nothing true to say about
it.

The HTTP path stays with the engine because `Sec-Fetch-Site` is strictly better
there. It tells same-site from cross-site, which a host compare cannot, and it
is written by the browser rather than computed by a proxy.

## Consequences

- A call from Chrome or Firefox through the gateway works. It never did.
- A future WebSocket route cannot read `Origin` behind the gateway, which
  consumes it, and `api::browser_origin` says so. `Sec-Fetch-Site` still crosses
  the hop, so the engine's gate is not inert for a WebKit socket: it decides
  there on its authoritative arm, and a test pins that the header is forwarded.
- Two copies of `origin_authority_matches_host` exist. The gateway has no
  dependency on the engine on purpose, and `is_hop_by_hop` is the same trade.
- **Both copies were tightened, twice.** The hostname-only arm ignored the
  origin's port, so a portless `Host` matched the same name on ANY port. Two
  gateways run on one machine, each with its own serve route, so that was the
  very bypass the new check is for. It also ignored the scheme, so `http://name`
  on port 80 matched an https gateway on 443. The arm now takes an https origin
  on its default port and nothing else.
- **A plain-http front proxy on port 80 would break the check**, since a
  portless `Host` is read as 443. No shipped topology sends one: dev, the
  packaged app and Route A all carry a port, and Route B is
  `tailscale serve --https=443`.
- A front proxy that rewrites `Host` would break the check. `tailscale serve`
  passes the client's through, which is the shipped remote-access route.
- ADR 0151's consequence bullet about browser support is corrected in place.

## Alternatives considered

**The gateway injects `Sec-Fetch-Site` on the upgrade hop**, computed from
`Origin` against `Host`, owning the inbound header there. The engine's existing
rule would then decide and log, unchanged, leaving one decision point rather
than two. Rejected for one reason: it makes `Sec-Fetch-Site` a header the
gateway sometimes writes, and `browser_origin` leans on it being one only a
browser can set. A reader of the engine could no longer tell a browser's word
from a proxy's. This was the closest call, and it was put to the maintainer.

**The gateway injects `x-forwarded-host` and the engine compares against it.**
Rejected: it reopens a header `proxy.rs` deliberately strips and never
re-injects. Direct to the engine a legacy browser could then forge it and defeat
the gate. Closing that would need the machine-local token checked inside
`browser_origin`, which means state in a layer that has none.

**Widen the engine's fallback to accept any loopback origin.** Rejected
outright. A page on another localhost port is precisely the attack the gate
exists to stop, and `another_port_on_this_machine_is_refused` pins it.

**Leave it, and tell people to use Safari.** Rejected. The desktop client is
Chrome for most of its users, and the packaged app's webview points at the
gateway too.
