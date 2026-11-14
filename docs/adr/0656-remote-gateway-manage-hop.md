# 0656: A device on gateway A manages gateway B's users and devices through a per-pairing grant, off by default

- **Status**: Accepted
- **Date**: 2026-11-11
- **Extends**: [ADR 0590](0590-gateway-pairing-is-a-deliberate-grant.md), [ADR 0591](0591-the-relay-is-the-one-proxied-hop.md)

## Context

Inside a remote workspace (`/dev@cloud/`), Settings showed gateway A's
devices and users, never gateway B's. The relay (ADR 0591) carries workspace
routes only, and B's `/~/api/v1/auth/*` admits only B's own paired devices.
So a person on A could not revoke a lost phone on B, or add a user there.
They first had to pair a device with B itself.

## Decision

B serves a *manage hop* at `/~/api/gateway/v1/manage/*`, and A forwards
`/~/api/v1/remote-gateways/<name>/manage/*` to it. B serves the hop only when
its pairing record for A carries the **manage grant** (`may_manage`). The
grant is off by default, for new and existing pairings. B's user turns it on
from the Paired gateways row, or with `lucidos pair --gateway --manage`.

The hop serves an exact allow-list: list and revoke devices; list, add,
rename and remove users; add and remove an email; move a device to another
user. `GATEWAY_PROTOCOL` becomes 4, since protocol 3 added workspace calls.

## Rationale

- **The grant belongs to a pairing, never to a user.** ADR 0623 keeps users
  free of authority, and nothing here changes that. B's user trusts a machine
  when they pair it. Letting that machine change B's people is a second
  trust, so it gets its own switch.
- **Off by default.** Pairing a gateway grants workspace access only. An
  upgrade must not widen that grant silently.
- **An allow-list, not a pass-through.** Pairing codes, gateway pairings, the
  external address, TLS, updates and the control plane stay unreachable. A
  compromised A then cannot mint a device on B or pair a third gateway.
- **A claim kind of its own.** The manage claim reuses the relay claim's
  signing, but names the fixed prefix `/~/manage/`. `~` is never a workspace
  slug, so neither kind verifies on the other route.
- **A checks B's protocol lazily.** A stores B's protocol at pairing and at
  each rotation. When the stored number is below 4, A re-reads B's health
  before it refuses. A B updated since pairing then never reads as too old.

## Consequences

- B's log names each change made through the hop, with the device label and
  the gateway it came through.
- A maps B's 401 to a 502 with `reason_kind: remote-refused`. The browser on
  A then never reads B's refusal of A as its own sign-out.
- A relayed device ("Sam via A") cannot be revoked from B through the hop,
  and a gateway pairing cannot be ended through it. Both stay managed on B.

## Alternatives considered

- **A `/~@name/` route on A.** Rejected: `remote_route` takes `~` as an
  invalid slug, and the picker's namespace would need a second meaning.
- **A per-user permission.** Rejected by ADR 0623: users carry no authority.
- **Grant on by default.** Rejected: it would widen every existing pairing on
  upgrade, without B's user deciding.
- **Forward every `/~/api/v1/auth` route.** Rejected: one compromised A could
  then mint device pairings on B.
