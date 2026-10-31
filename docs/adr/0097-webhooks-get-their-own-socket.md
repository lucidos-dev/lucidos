# 0097: Inbound webhooks answer on a separate loopback listener, never the main surface

- **Status**: Accepted
- **Date**: 2026-08-19

## Context

A webhook is the one Lucidos surface a user may deliberately expose to the open
internet, with `tailscale funnel`. Everything else stays behind the tailnet and
behind pairing (ADR 0094).

Funnel maps a **port**, not a path. So "expose only `/webhooks/*`" is not
something funnel can express. Whatever listener the funnel points at, a public
caller can address every path that listener serves.

The main gateway surface serves the control plane at `/~/api/v1/control/*`,
which creates and deletes workspaces, and proxies `/<slug>/` into every
workspace API. Pointing funnel at it would put both one auth bug away from the
public internet.

## Decision

A second listener inside the gateway process, on its own port, carrying its own
`Router`. It has exactly one route, `POST /<slug>/<hook-id>`, and an explicit
404 for everything else, a wrong method included.

The port derives from the gateway's own, `+10`, so dev takes 5261 and packaged
5262 and the two coexist as their gateways do. `LUCIDOS_HOOK_PORT` overrides it
and `0` switches the socket off.

It binds **loopback**, like the surface it sits beside. Funnel proxies from this
machine, so it reaches loopback, and nothing else on the network can address the
socket directly.

The listener **forwards and decides nothing**. It resolves a slug to an engine
port and streams the body through untouched. The engine verifies and emits.

## Rationale

Structure beats configuration for a public surface. A path allow-list on a
shared listener is a rule someone can get wrong, and a route added later
inherits the exposure by default. A separate router cannot answer a path it does
not have.

Auth lives with the secret. The engine owns the `credentials` table. Verifying
in the gateway would mean copying a secret into a second process, which the user
ruled out. It would also mean the gateway growing a database handle, which ADR
0014 §1 keeps it without.

The body reaches the engine byte-for-byte because an HMAC is computed over
exactly those bytes. Any reserialization at the hop would break every signed
sender, so the forward streams and never parses.

A public port needs limits the private one does not: a 1 MB body cap, a request
timeout, a concurrency ceiling, and `catch-panic`. The gateway unwinds rather
than aborting, and it supervises every workspace. An unhandled panic on the hook
path would take the machine's only gateway down with it.

Nothing about the socket is fatal to the gateway. A hook port already in use
logs and is skipped; a listener that dies later logs and stops answering. The
gateway's job is serving workspaces, and a webhook surface is not worth failing
that.

## Consequences

- Exposing webhooks publicly is `tailscale funnel <hook-port>`, and it can reach
  nothing else. The control plane and every workspace stay unreachable by
  construction rather than by rule.
- A delivery URL is `{host}:{hook_port}/<slug>/<hook-id>`, mirroring the
  workspace-address convention.
- The gateway's dependency list grows two `tower-http` features
  (`catch-panic`, `timeout`) and one `tower` feature (`limit`). Its manifest
  keeps a short list on purpose, and these are recorded there with the reason.
- An unknown slug, a stopped workspace and a disabled hook all answer the same
  404 or 401, so probing tells a public caller nothing.

## Alternatives considered

**A path on the main gateway listener.** Rejected. Funnel maps a port, so
exposing that path exposes the control plane and every workspace with it.

**One public port per workspace.** Rejected on arithmetic. Funnel offers 443,
8443 and 10000, and Serve already wants one for the main surface. That runs out
at two workspaces, so routing by slug on one listener is forced.

**Verify in the gateway, forward only what passes.** Rejected. It copies the
webhook secret out of the engine's `credentials` table, which the user ruled
out. It also gives the gateway a database dependency ADR 0014 keeps it free of.

**Parse the body at the gateway to route on its content.** Rejected. A signature
covers the raw bytes, and a parser is the wrong thing to run for a caller that
has not been authenticated yet.

**A separate process for the hook listener.** Rejected as premature. It buys
crash isolation that `catch-panic` already provides for the realistic failure,
and costs a third supervised process on every install.
