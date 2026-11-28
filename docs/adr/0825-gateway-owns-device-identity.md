# 0825: The gateway owns device identity: one name per device, workspaces keep push, presence and subscriptions

- **Status**: Accepted
- **Date**: 2026-11-27

## Context

The device registry existed twice. The gateway's paired-device store held a
device's id, pairing label and user. Every workspace database held a `devices`
table with a typed name, the pairing label again, the user agent, push and
presence. A rename in one workspace did not show in another, and the Devices
page merged two sources. Design decision 7 of
`docs/plans/2026-11-26-gateway-and-workspace-settings-split-design.md` gives
identity to the gateway.

## Decision

The gateway is the only store of a device's typed name and pairing label. The
`devices` table keeps only what is the workspace's own: push on or off, last
seen, the user agent, and the rows that hang off it (subscriptions, presence,
preferences, pins). Every engine resolves names from the gateway's list.

- The gateway's event log records `DeviceNamed` for any device id: paired,
  local or relayed. A relayed device's pairing label rides on
  `RelayedUserSeen`.
- The gateway serves `GET /~/api/v1/auth/device-names` and
  `PUT /~/api/v1/auth/devices/:id/name`, to a paired device and through the
  manage hop. `POST /~/api/v1/auth/device-names/import` takes a local process
  only.
- The engine caches the list in memory, and reads it again when the gateway
  signals an identity change. It announces each changed name as
  `DeviceRenamed`. A rename through the engine goes to the gateway.

## Rationale

- **One name per device.** A rename happens once and shows in every
  workspace, which is the user-visible point of the decision.
- **The engine stays stateless.** The cached list is rebuilt from the gateway
  at boot, so a restart loses nothing.
- **The user agent stays per workspace.** The design does not list it as
  identity. It is what the browser told this workspace, and native-push
  detection reads it. The fallback label ("Chrome on Mac") is computed from
  it, so it reads the same in every workspace.

## Consequences

- **The move keeps every name.** The migration copies `name` and
  `pairing_label` into `device_name_imports` before it drops the columns. The
  engine hands those rows to the gateway at boot, and deletes each only after
  the gateway accepts it. A gateway that is down only delays the hand-over.
- **The first imported name wins.** Two workspaces can hold different names
  for one device. The gateway keeps the first it receives. A later import
  never overwrites a name, so a workspace booting late cannot undo a rename.
  A pairing label becomes the name only where the gateway knows no label for
  that device.
- **Revoking a device forgets its name**, and ending a gateway pairing forgets
  the names of its relayed devices.
- **An engine without a gateway shows fallback names.** Its held names wait in
  the table for a boot that has a gateway.
- **The one-off device sweep waits for the gateway's list**, since a named
  device is never a one-off one. It skips a run while the list is unknown.

## Alternatives considered

- **Keep the names in each workspace and sync them.** Two writers of one fact
  disagree eventually, and nothing says which wins.
- **Resolve every name with a live gateway call.** Names are read on hot
  paths: every event label, every push and every device row. A cached list
  refreshed on the gateway's signal costs one read per change.
- **Copy names to the gateway inside the SQL migration.** A migration cannot
  call the gateway. Dropping the columns before the gateway confirms would
  lose names whenever it is down.
- **Last import wins.** A stale workspace that boots after a rename would undo
  that rename.
