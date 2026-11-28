# 0830: The admin grant and hop: a paired gateway's devices manage it as a local device would, amending 0656

- **Status**: Accepted
- **Date**: 2026-11-27
- **Amends**: [0656](0656-remote-gateway-manage-hop.md)

## Context

The Settings **Gateway** group names the gateway its pages show, and the
gateway switcher picks a remote one (design
`docs/plans/2026-11-26-gateway-and-workspace-settings-split-design.md`).
ADR 0656 kept the rest of B's administration off every hop: the manage hop
reaches B's users and devices, and the control hop (ADR 0811) B's workspaces.
So B's access, update and gateway-pairing settings still needed a device
paired with B itself.

## Decision

A third grant on B's pairing record for A, `may_admin`, opens an admin hop.
Through it a device on A reaches the Gateway group's routes on B, gateway
pairing included, as a device paired with B would. The admin grant implies
the manage and control grants at B's check.

- B serves the hop at `/~/api/gateway/v1/admin/`, laid out like its own
  `/~/api/v1/`. A forwards `/~/api/v1/remote-gateways/<name>/admin/*`.
- The allow-list covers the gateway's status, reload, update check, release
  check and installs, plus the engine builds and their switch (ADR 0824). It also covers network config, external address, TLS
  and tailnet status. Last come devices, users, gateway pairing secrets, the
  paired-gateway list and unpair.
- No grant switch is on any hop. A secret minted through the hop cannot
  start with any grant: admin, manage or control.
- A secret minted through the hop records `Minter::RemoteDevice`, so it
  redeems without an Allow on B. B logs the device and the gateway it came
  through.
- `GATEWAY_PROTOCOL` rises to 7. An older B answers `remote-gateway-too-old`.
- **Pair a gateway** ticks the three grants before minting, and
  `lucidos pair --gateway --admin` starts a pairing with admin on.

## Rationale

Parity is what the user asked for: a person who owns both gateways should
not need a second paired device to change B's settings. Keeping it a
separate grant keeps the two narrower ones meaningful. An owner can still
share B's workspaces without sharing B's network settings.

The grant switches stay off the hop because a grant set from A could not be
taken back from A. With them on B only, B's own devices stay the authority,
and turning admin off on B ends A's reach at once. Refusing every grant tick
on a hop mint closes the same loop one step out. A cannot pair a third
gateway, or itself again, with a grant on B that outlives A's own.

A hop mint needs no Allow because the caller already holds B's broadest
grant. Requiring an Allow on B would make the parity a fiction.

## Consequences

- Existing pairings read `may_admin = false`, so nothing gains reach on
  upgrade.
- A device on A with admin can unpair any gateway from B, A included.
- Pairing-request answers, device pairing codes and B's own remote-gateway
  list stay off the hop. Their handlers need a device paired with B, and
  remote device onboarding is a separate decision.
- The Gateway group's Access and Updates & Installs pages do not call the hop
  yet. They still say to open them on B itself.

## Alternatives considered

- **Widen the manage hop.** One grant would then cover users and network
  settings together, and an owner could no longer share one without the
  other.
- **Let the admin grant be set from A.** Simpler for one owner, but a
  compromised A could then keep itself in, which is the case a grant exists
  to stop.
- **Treat a hop mint as a local process.** It would wait for an Allow on B,
  which needs the device on B that the admin grant exists to make
  unnecessary.
