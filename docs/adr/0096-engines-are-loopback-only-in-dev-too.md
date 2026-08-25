# 0096: Engines bind loopback in dev as well as packaged, so the gateway is the only network path

- **Status**: Accepted
- **Date**: 2026-08-19

## Context

ADR 0094 made the gateway authenticate every network caller. A device pairs, and
an unpaired one is refused.

Dev did not honour that. `start_gateway` set `LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0`,
so the gateway spawned every engine bound to all interfaces. A phone on the
tailnet could open `https://<host>:5173/` and get a whole workspace with no
credential: every thread, every credential, every coding-agent capability.

ADR 0014's dev-topology table made that normative. It read "loopback-only is the
PACKAGED posture, NOT dev", and `.claude/rules/dev-runtime.md` said "never make
the dev engine loopback-only". Both were written before there was anything to
bypass.

## Decision

A gateway-spawned engine binds loopback, in dev exactly as packaged. The gateway
is the only network path into a workspace. `LUCIDOS_GATEWAY_ENGINE_LOOPBACK`
still exists and still reopens the old topology, but nothing sets it.

The engine additionally refuses a cross-origin browser request across all of
`/api/v1` (`api::browser_origin`). Loopback stops a remote caller, not a page on
another origin driving that port out of the user's own browser.

## Rationale

Auth that one URL walks past is not auth. The engine port was not a lesser door:
it served the same API, the same app UIs and the same data, and it was the door
a returning phone had bookmarked.

The direct port existed so the app was reachable from a phone. The gateway now
does that, at `/<slug>/`, and it authenticates. So the reason for the exception
is gone rather than outweighed.

Nothing new was built for this. Packaged has spawned loopback engines since ADR
0014, `engine_tls` already follows `engine_loopback`, and `stack.rs` already
strips the TLS cert on that branch. Dev now takes the tested path instead of its
own.

The browser gate is likewise a move, not an invention. The engine already
carried the policy for `/proxy/*`, where a hostile page could otherwise trigger
a credentialed upstream call. That reasoning covers the whole API surface.

## Consequences

- A device reaches a workspace at `https://<host>:5251/<slug>/` and pairs. A
  bookmark pointing at an engine port stops resolving from elsewhere, which is
  the point, but the user feels it on the next reload.
- The gateway proxies and health-probes over http rather than https in dev, the
  packaged behaviour.
- `https://localhost:5173/` still works ON the machine, so the engine's own port
  stays usable for debugging.
- ADR 0014's dev-topology table and `.claude/rules/dev-runtime.md` both changed,
  because both said the opposite.
- A deployment that genuinely wants the old shape sets
  `LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0` and takes the bypass with it.

## Alternatives considered

**Leave dev alone and document the hole.** Rejected. The development topology is
where the next feature gets tested, and it runs on a tailnet-exposed machine. A
documented bypass there is the one most likely to be relied on.

**Keep the port open and put pairing on the engine too.** Rejected. It
duplicates the authorizer into a second crate, and ADR 0014 §1 keeps the engine
out of the network-facing role deliberately. Two implementations of one auth
decision is how they drift.

**Bind the engine to the tailnet address only.** Rejected. It narrows the
exposure without removing it, and it makes the bind depend on Tailscale, which
ADR 0094 keeps out of the auth path entirely.

**Copy the gateway's `control_request_allowed` into the engine.** Rejected for
the browser gate. It refuses an app-iframe `Referer`, which is right for a
destructive control plane and wrong for the engine: apps call the engine API
through the SDK from inside an iframe, and the Phase 1 plan keeps them working.
The engine's own shipped policy was already the correct one.
