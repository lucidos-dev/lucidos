# 0155: An engine that faces a network requires a scoped local credential; the gateway's control plane was already authenticated

- **Status**: Accepted
- **Date**: 2026-08-29

## Context

A nightly security review reported that a LAN or tailnet caller could drive the
gateway control plane with no credential. `control_request_allowed` treats a
request with no `Origin`, no `Referer` and no `Sec-Fetch-Site` as a trusted
non-browser client. So the report had workspace create, stop, delete-to-trash,
backup restore and gateway reload all following from that. It was scored
CRITICAL, and it had been raised three times.

**The claim was already false.** ADR 0094 put `auth_api::enforce` in front of the
whole gateway router. Both gateways on the maintainer's machine were measured,
one of them bound to a tailnet address. An uncredentialed control request is 401
on every bind, and only the machine-local token or a paired device gets through.

It kept being re-raised because `control.rs` said otherwise. Its module note
claimed a header-less caller was "protected instead by the gateway's loopback
bind", and the unit tests underneath exercised the CSRF helper alone. So a
reader saw a function returning `true` for a bare curl, beside a comment
agreeing that this was the only gate.

The other half of the report was live. The engine had no request-level
authentication at all. `api::browser_origin` said so in its own module doc: "The
API is unauthenticated by design, on the premise that reaching it proves you are
local." Four settings retire that premise: `LUCIDOS_BIND_ALL`,
`LUCIDOS_BIND_ADDR`, `LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0`, and
`~/.lucidos/network.toml` for a directly-launched engine. The picker's own
Network access control writes the fourth.

## Decision

Three things.

**An engine whose bind is not loopback requires a credential**, on every path
but `/api/v1/health`. On a loopback bind it requires nothing, so every shipped
topology is unchanged. The credential is the machine-local token that already
exists (ADR 0094), read through `lucidos-local-token`.

**The credential carries a scope.** The full-authority token reaches everything.
A second token, `~/.lucidos/webhook-token`, reaches exactly
`/api/v1/webhooks/<id>/deliver`. The gateway's hook socket presents it, and
nothing else does.

**Both header names belong to the gateway**, like `x-lucidos-device-id` before
them (ADR 0050). The proxy strips an inbound one and re-injects its own.

`control_request_allowed` keeps its shape. It is a CSRF check, and behind
`enforce` its header-less arm is correct. Only its stated reason changed, and a
router-level test now pins the refusal it was wrongly credited with.

## Rationale

The bind is what makes a loopback engine safe, and nothing stood behind it. That
is defensible while the bind cannot be widened, and it can: the widening is a
control in the product's own settings. A security property resting on a
configuration the UI offers to change is not a property.

The scope exists because of one caller. A hook socket is the surface a user may
point `tailscale funnel` at (ADR 0097). It is the one hop whose input comes from
the open internet. Handing it full authority would put the whole engine API one
forwarding bug behind a public port. Two files rather than one token plus a
scope field, because a scope the caller states is not a scope: the bearer would
state the widest one.

The gate wraps the OUTERMOST router. `/app` and `/data` are siblings of the
`/api/v1` nest, so a gate inside that nest would miss both. `/data` serves the
workspace's files straight off disk. Splitting reads from writes was rejected
for the same reason: a read of `/data` is the leak.

Reusing the local token beats minting an engine-specific one. It already exists,
the CLI already sends it to the engine, and one secret with one owner cannot
drift from a second copy.

Comments are the load-bearing part of the first change. This finding cost three
review cycles because a true statement had quietly become false and no test
noticed. `gateway_router` is split out of `serve` so a test drives the real
router, not a restatement of the route table.

## Consequences

- A browser cannot reach a wide-bound engine directly. It comes through the
  gateway at `/<slug>/` and pairs. ADR 0096 already stated that posture, and
  this enforces it rather than assuming it. Pointing a phone at an engine port
  is retired with the legacy no-gateway route.
- A loopback engine is untouched. Dev, e2e, apps, the desktop app and the PWA at
  `https://localhost:5173` all behave exactly as before, because the middleware
  returns immediately.
- The gateway mints a second 0600 file at boot, and treats a failure as fatal on
  the same terms as the first.
- `LUCIDOS_PERMISSIVE_CORS` disables the origin gate and NOT this one. An escape
  hatch for one concern must not silently open another.
- Wrong pairing guesses are now capped at ten a minute. A throttled attempt is
  429 rather than 400, so a person mid-pairing is not sent hunting a typo. The
  cost is a caller spamming the public pair route can stall a real redemption
  for up to a minute. A correct code clears the budget, so a typo never does.
- `lucidos-local-token` grows a named-token API and stays dependency-free.

## Alternatives considered

**Leave the engine as it is and rely on the loopback default.** Rejected. It is
the status quo that produced the finding, and the widening controls ship in the
product.

**Refuse to bind non-loopback at all, and delete the four widening settings.**
Tempting, and the smallest diff. Rejected because `LUCIDOS_NO_GATEWAY=1` is a
real launch mode and the maintainer asked for authentication rather than
removal. It would also make the per-workspace Network access setting a lie in a
second way rather than the first.

**Put pairing on the engine too.** Rejected again, for ADR 0096's reason: it
duplicates the authorizer into a second crate, and two implementations of one
auth decision drift. The engine learns one machine-local secret and no more.

**Gate only the mutating and credentialed routes.** Rejected. Every thread,
artifact and revealed credential is a read, and `/data` is a read. The split
buys nothing and invites a new route to land on the wrong side of it.

**One token with a scope field in the header.** Rejected. A scope the caller
names is chosen by the caller.

**Derive the webhook token from the local one by HMAC.** Rejected. It needs a
hash in `lucidos-local-token`, which is deliberately dependency-free, or the
same derivation copied into two crates that must not disagree.

**Make gateway auth conditional on the bind, matching the engine.** Rejected as
a regression. ADR 0094 requires a credential on every gateway bind, because
`tailscale serve` proxies from this machine and a loopback peer address
therefore proves nothing.

**Cap pairing attempts per code instead of per window.** Rejected. It lets an
attacker burn the user's outstanding code, and it does not bound the total guess
rate when several codes are live.
