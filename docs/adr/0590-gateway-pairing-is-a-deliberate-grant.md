# 0590: Gateway pairing is a deliberate, one-way grant recorded on both sides

- **Status**: Accepted
- **Date**: 2026-10-27
- **Extends**: [0132](0132-gateway-auth-state-is-per-gateway.md)

## Context

Remote gateways (design `docs/plans/2026-10-26-remote-gateways-design-in-progress.md`,
points D1 to D4, D12 and D13) let gateway A relay gateway B's workspaces to A's
own devices. Before any relay exists, the two gateways need a trust
relationship. ADR 0132 keeps auth state per gateway: two gateways on one
machine never share a devices file. A remote gateway must not erode that by
reading B's store or by holding a device credential it then hands around.

## Decision

A *gateway pairing* is an explicit grant from B to A. A code makes it, and
each side records it in a file of its own.

- **B mints a gateway pairing code on its own code kind.** Settings → Access →
  "Pair a gateway", or `lucidos pair --gateway`. Gateway codes live apart
  from device codes. So a device code never pairs a gateway, and a gateway
  code never pairs a device.
- **A redeems it over `https` only.** A probes B's public health first. It
  refuses plain `http`, an answer that is not a gateway, a gateway older than
  protocol 1, and itself.
- **Redeeming returns a gateway credential and a pairing id.** B stores the
  credential's digest in `<data dir>/paired-gateways.json` and lists it with
  Unpair. A stores the credential in `<data dir>/remote-gateways.json`, owner
  only, because A must present it.
- **A prefers B's external address.** B returns its ADR 0550 address in the
  redemption answer when it is `https`. A keeps it only when A reaches B
  there, proved by the same `instance_id`. Otherwise A keeps the typed one.
- **Gateway-to-gateway calls live under `/~/api/gateway/v1/`.** The device
  gate does not apply there. That router authenticates the gateway credential
  itself, in the `x-lucidos-gateway-credential` header, and answers 404 for
  anything it does not route.
- **B's health names its protocol version** as `gateway_protocol`. The path
  and the version only ever add (D12).

## Rationale

The code is the grant. Somebody holding B's Settings chose to let A in, and
A's user typed what B showed them. That mirrors device pairing (ADR 0094),
which users already understand.

A separate code kind keeps the two grants from blurring. A device code sent
to a phone must not become standing access for a whole other gateway. The
reverse would put a machine credential in a browser jar.

A gateway credential is not a device credential. It never authorizes a device
route, and a device cookie never authorizes a gateway route. The relay (phase
5) adds a signed claim on top, so B sees A's calling device without ever
holding A's devices file. Auth state stays per gateway, as ADR 0132 requires.

`https` is mandatory because the credential, and later every device claim,
crosses a network. A plain `http` pairing would put both on the wire in clear
text.

## Consequences

- Pairing is one-way. Access both ways means pairing twice (D3).
- The grant is one-way, and the trust is not. B's pages run on A's origin
  once A relays them, so a paired B can act as any of A's devices on A (ADR
  0591). A should add only a gateway its own user runs, and A's form says so.
- A gateway serving plain `http` only cannot be a remote gateway. A's form
  says so and names the fix: remote access with `tailscale serve`, or an
  external address.
- Unpair on either side ends the grant. B forgets the digest, so A's next
  call is refused. A removing B also asks B to forget it, best effort.
- A's file holds live credentials. It is written owner-only, like the local
  token, and never returned by any API.

## Alternatives considered

- **Reuse a device code and device credential for A.** Rejected: B could not
  tell its own phones from a gateway acting for many devices, and Unpair on B
  would list A as one more device.
- **Share one devices file.** Rejected by ADR 0132: shared auth state is how
  one gateway's writes deleted the other's devices.
- **Allow `http` on a trusted network.** Rejected: "trusted" is a guess about
  a network Lucidos cannot see, and a leaked credential is standing access.
- **Publish B's external address in public health.** Rejected: the
  redemption answer is gated by the code, so only a gateway the user chose
  learns it.
