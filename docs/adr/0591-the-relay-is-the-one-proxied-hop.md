# 0591: The relay is the one hop that proxies another gateway's workspaces, and only for a gateway pairing

- **Status**: Accepted
- **Date**: 2026-10-28
- **Extends**: [0590](0590-gateway-pairing-is-a-deliberate-grant.md), [0014](0014-multi-workspace-redesign.md)

## Context

`peers.rs` says that between two gateways "a location crosses, and nothing
else". No credential is forwarded and nothing is proxied, because ADR 0132
refused to widen a pairing minted on one gateway into authority over another.

Remote gateways (design points D2, D5 and D6) need exactly that widening. A's
devices must open B's workspaces under A's own origin. ADR 0590 made the
widening an explicit grant: B's user mints a gateway pairing code and A
redeems it. This ADR records how A then carries its devices into B.

## Decision

Gateway A relays `/<slug>@<name>/*` to gateway B's
`/~/api/gateway/v1/relay/<slug>/*`, where `<name>` is the short name A gave B.
This is the only hop that proxies another gateway's workspaces, and it exists
only for a gateway pairing.

- **A signs every request.** It sends its gateway credential and a claim: the
  claim version, the issue time, the outside prefix `/<slug>@<name>/`, and the
  calling device's id and label. An HMAC-SHA256 keyed by the credential signs
  the claim.
- **B verifies before anything else.** A current pairing's credential, a valid
  signature, an issue time within five minutes, and an outside prefix that
  names the slug in the path. Unpair revokes on the next request, because B
  reads its pairing store on every one.
- **B namespaces the device.** B's engine sees `<pairing id>:<device id>`, so
  a relayed device never collides with one of B's own. Only B's gateway writes
  the device and prefix headers. Client-sent ones are stripped on both hops.
- **B serves the request like its own.** It rewrites the path to `/<slug>/*`
  and runs its ordinary workspace route: lazy start, boot pages and the engine
  proxy. The engine reads the outside prefix from `X-Forwarded-Prefix`, so the
  page base is `/<slug>@<name>/` with no engine change.
- **A claim with no device reaches only what B admits without one.** That is
  a public app asset or a frame capability B verifies. A forwards a
  capability-bearing path unverified, because B's engine minted the
  capability and only B holds its key.
- **The relay streams like the local proxy (ADR 0014).** No total timeout, no
  decompression, no HTML rewrite, and hop-by-hop headers stripped both ways.
- **The credential is one-way, and the origin is not.** A gateway credential
  authorizes nothing on the gateway that holds it, so B cannot call A. But
  B's pages run on A's origin, in the browsers of A's devices. Their script
  can do anything those devices can do on A. So A trusts a paired B like
  itself. A's "Add a remote gateway" form says to add only a gateway you run
  (amended after review).
- **WebSocket upgrades are not relayed.** A answers 501 with the reason.
  Voice is the only WebSocket user, so a relayed workspace has no voice: the
  call toggle hides there, and Settings says why (amended after review).

## Rationale

The pairing is the consent. B's user chose to let A in, so A's devices
inheriting B's workspaces is what they granted. Every other gateway-to-gateway
case, two installs on one machine included, keeps navigating as `peers.rs`
does.

The signed claim keeps B in charge of its own engine's view. B decides what
device id its engine sees and refuses a prefix pointing anywhere but the
relayed slug. B never reads A's device store, so auth state stays per gateway
(ADR 0132).

`@` cannot appear in a slug (`registry::is_valid_id`), so a remote route never
shadows a local workspace and no local route changes.

## Consequences

- A's devices see B's frontend, served by B's engine, at A's origin. Links,
  notifications and the workspace list follow in later phases of the plan.
- A relayed workspace's voice calls need a WebSocket relay, which does not
  exist yet.
- B's engine sees one device row per relayed device per pairing. Unpairing
  leaves those rows as history.

## Alternatives considered

- **Navigate to B's origin, as `peers.rs` does.** Rejected: every device would
  need pairing with B separately, which is the chore D1 removes.
- **Forward the device's cookie to B.** Rejected: B does not know A's devices,
  and teaching it would share auth state across gateways.
- **A device credential for A on B.** Rejected in ADR 0590: a machine
  credential in a device store blurs the two grants.
- **Trust headers without a signature.** The credential already authenticates
  the hop over `https`. The signature is cheap, binds the claim to the
  credential, and bounds a captured request's replay to five minutes.
