# 0806: Agents query sanitized gateway and desktop client logs through the local gateway.

- **Status**: Accepted
- **Date**: 2026-11-24
- **Amends**: [ADR 0667](0667-agents-read-a-sanitized-diagnostic-log.md) and
  [ADR 0716](0716-diagnostic-bundles.md).

## Context

The agent query originally covered one workspace engine. Gateway and desktop
client failures left useful evidence in logs the agent sandbox hides.

## Decision

Agents query the engine, gateway and desktop client via
`lucidos diagnostic-log query`. The engine verifies its agent's origin token
and asks the gateway over the local-token control route. Only the gateway
opens the gateway and desktop client logs.

The gateway serves its own sanitized log to authorized workspace agents on
personal and team gateways. The current team owns the retained gateway log,
including records about former members. Offboarding revokes the removed user's
access through their devices. The gateway renders lines for the owner and masks
secrets again at read time.

A packaged macOS gateway may serve structured startup and relaunch records from
the private data directory it shares with its desktop client. A gateway with
multiple users or any incoming pairing reports `team-gateway` for that source.
The gateway checks that restriction before and after the scan.

## Rationale

Gateway records are shared operational evidence for the gateway's current
users. One desktop process can connect to several gateways. Typed lifecycle
records describe the packaged client's own startup and relaunch, and the
data-directory check associates those records with its managed gateway.

## Consequences

- The query merges sources chronologically and reapplies the requested budgets
  to the whole timeline.
- Each line names its source. The response reports availability for every
  requested source, including unsupported platforms and older gateways.
- The engine scan has its own concurrency limit. A busy engine source still
  lets a combined query reach the gateway.
- Older gateways may reply with a 404 or the HTML workspace picker. Both
  replies report `gateway-too-old`.
- The diagnostic bundle still captures the existing desktop and relaunch
  output logs.
- Sending a diagnostic bundle still requires the owner's consent. The sandbox
  and raw host-log transfer rules stay in force.

## Alternatives considered

Opening the files through the engine or agent would duplicate the gateway's
path and authority checks. Requiring a bundle for each local query would
interrupt routine debugging for the gateway's authorized agents.
