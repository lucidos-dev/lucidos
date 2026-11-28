# 0826: Engines bind loopback only: the network_bind preference, the inherit flag and the gateway's engine opt-out are gone

- **Status**: Accepted
- **Date**: 2026-11-27
- **Amends**: [0096](0096-engines-are-loopback-only-in-dev-too.md), [0155](0155-engine-requires-a-credential-on-a-wide-bind.md)

## Context

ADR 0096 put every gateway-spawned engine on loopback. Four ways to widen an
engine survived it:

- the per-workspace `network_bind` preference, edited in Settings,
- the `[engine] inherit` flag in `network.toml`, which made an engine copy the
  gateway bind,
- `LUCIDOS_BIND_ADDR` and `LUCIDOS_BIND_ALL` for a directly launched engine,
- `LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0`, the gateway's opt-out.

A gateway-spawned engine already ignored the first three, because the gateway's
`LUCIDOS_BIND_LOOPBACK` floor beat them. So the Settings section mostly edited
a value that could not apply. ADR 0155 then had to give a wide engine its own
credential gate, a second door beside the gateway's.

The gateway and workspace settings split (design decision 6) gives every
setting one owner. Host exposure belongs to the gateway.

## Decision

Every engine binds loopback, whatever the environment, `network.toml` or the
database says. The gateway bind on Gateway → Access is the only network
setting. A workspace with a stored wide `network_bind` logs once at boot that
the value is ignored.

## Rationale

- **One door.** The gateway authenticates every network caller (ADR 0094). An
  engine port facing a network is a second door, and ADR 0155's gate existed
  only to guard it.
- **The setting rarely applied.** Packaged and dev gateways always pinned
  their engines to loopback. Only a directly launched dev engine honoured it.
- **No reach is lost.** The gateway bind gives the same addresses, through
  pairing.

Two checks ran before removal, recorded in phase 4 of
`docs/plans/2026-11-26-gateway-and-workspace-settings-split.md`. No preview or
direct-port path needs a non-loopback engine. No stored value on the
maintainer's machine applied.

## Consequences

- `network_bind` is a retired preference key. A write is refused with a
  reason, and the engine reads a stored value only for the boot log.
- The `[engine] inherit` line in an older `network.toml` is ignored. The next
  save from the picker rewrites the file without it.
- The engine `/api/v1/network-config` route is gone, along with the engine copy
  of `/api/v1/tailnet-status`. That bumps the workspace-client contract to 2
  (ADR 0723).
- ADR 0155's credential gate is gone. The engine keeps only its loopback Host
  check against DNS rebinding. The local token still attributes a local
  process, and the gateway still mints the hook socket token for itself.
- `LUCIDOS_GATEWAY_ENGINE_LOOPBACK`, `LUCIDOS_BIND_ADDR` and `LUCIDOS_BIND_ALL`
  no longer do anything for an engine. The last two stay reserved names in the
  environment store.
- `LUCIDOS_BIND_LOOPBACK` keeps its name. It now means only "behind a
  gateway", which picks the API base URL handed to subprocesses.

## Alternatives considered

- **Rename `LUCIDOS_BIND_LOOPBACK` to say "behind a gateway".** Rejected for
  now. A gateway and an engine from different builds meet during an upgrade.
  An engine that missed the old name would hand its subprocesses the wrong
  base URL.
- **Keep ADR 0155's gate as defence in depth.** Rejected. With no wide bind it
  never runs, so it is dead code that reads like a live door.
- **Keep the gateway's engine opt-out for unusual deployments.** Rejected. It
  reopened exactly the bypass ADR 0096 closed, and nothing in the tree set it.
