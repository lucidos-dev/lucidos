# 0719: Durable gateway identity is private to authenticated pairings

- **Status**: Accepted
- **Date**: 2026-11-16

## Context

A gateway can have several HTTPS addresses. Pairing the same destination under
different names creates duplicate workspace listings and grants. Neither names
nor addresses identify the destination. Public health's `instance_id` changes
on every boot deliberately (ADR 0602).

## Decision

Persist a random 256-bit gateway identity in `gateway-identity` in the gateway's
data directory. Publish it only when redeeming a pairing secret and through the
authenticated own-pairing endpoint. Keep public health and its boot identity
unchanged. Store the destination identity alongside outgoing pairings as an
optional field, so existing stores and older peers remain readable.

## Rationale

The data directory defines the gateway across process restarts, independent of
hostnames, ports, labels, installation type and network topology. A random id
reveals no machine or workspace information. Authenticated disclosure avoids
adding a durable public tracking identifier. Atomic publication without
replacement makes concurrent first loads agree; corrupt or unreadable state
must fail rather than silently change the gateway's identity.

## Consequences

Uniqueness enforcement can compare durable destinations across aliases and
restarts. Missing identities on existing pairings require authenticated
hydration or explicit legacy evidence before adding another destination.
Existing duplicates remain user-managed. Backups must preserve the identity
file along with the rest of the gateway's state. Copying that state represents
the same logical gateway.

## Alternatives considered

- Persist the public `instance_id`: changes ADR 0602's boot-scoped proof and
  exposes a durable public identifier.
- Compare normalized URLs only: cannot recognize aliases.
- Derive identity from hostname, directory or workspaces: leaks information,
  collides across installations and changes when mutable properties change.
- Overwrite invalid identity files: silently breaks destination comparisons.
