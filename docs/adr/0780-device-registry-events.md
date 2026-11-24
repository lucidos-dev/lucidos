# 0780: The device registry emits one event per write and keeps cascades silent under a single announcement

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0032](0032-a-state-write-owns-its-announcement.md)

## Context

The device registry lists the devices that have connected to the workspace. A
browser used to take its device id from `localStorage`. It now takes its id
from the workspace gateway.

Without a migration, such a browser arrives as a new device. It then loses its
push subscription and its preferences. During one page load the browser knows
both ids, and that is the window the migration uses.

This record restates the design comments of
`crates/lucidos-core/src/core/devices.rs`, written before the comment strip
removes them. Nobody checked it line by line against the code.

## Decision

- `register`, `rename`, `set_push_enabled`, `delete`, `remove_one_off` and
  `hand_over` are the only reachable mutators. The raw row writes are private
  to the module. Each mutator emits `DeviceRegistered`, `DeviceRenamed`,
  `DevicePushChanged`, `DeviceDeleted` or `DeviceHandedOver`. These events
  reload the Settings devices list on every other device.
- The helper that deletes a device and everything scoped to it is private. It
  runs inside the caller's transaction. Only `delete` and `remove_one_off` call
  it, and they emit.
- The delete cascade is silent. It covers per-device preferences, push
  subscriptions, pinned apps and presence. `DeviceDeleted` announces all of it.
- `hand_over` moves every trace of the old id onto the new id and announces one
  event for the whole move. It follows the same cascade rule as `delete`.
- If the new id already has a row, the two rows merge. The new id's own
  settings win. Everything it lacks comes over from the old id.
- The order is fixed. Make sure the new row exists, repoint the children, and
  drop the old row last.
- The old id becomes an alias of the new id. History recorded under it resolves
  to the device's current name through `friendly_name`.
- `push_log` is left alone. It records what was sent to which id at the time,
  so it is history, like an event payload.
- The hand-over result reports `AlreadyDone` and `NoSuchDevice` as ordinary
  outcomes, not errors.
- Preferences and pinned apps are written through their owning stores, not by
  this module.

## Rationale

- `push_subscriptions.device_id` is a foreign key onto `devices(id)` with no
  `ON UPDATE CASCADE`. The parent key cannot be renamed in place. For the same
  reason, the children go first when a device is deleted.
- Per-row events such as `PreferencesChanged` or `PinnedAppUnpinned` would
  describe changes to a device no client still tracks. One device-level event
  is enough.
- This module owns neither the preferences table nor the pinned apps table.
  `core::announced_surfaces` exists to refuse a module writing another module's
  table behind its back.
- A client retries a hand-over on a later load. By then the first attempt has
  usually landed. So the caller can answer without guessing, and a retry is not
  an error.
- The registry has the same shape as `RepositoryStore`, so no caller can skip
  the event.

## Consequences

- Other devices see one event per device change and reload the devices list.
- A browser that moves to a gateway-minted id keeps its push subscription and
  preferences.
- History recorded under the old id still shows the device's current name.
- `push_log` keeps the old id, as a record of what happened at the time.
- A hand-over retry after success or for an unknown device returns a normal
  outcome.
- Any new cascaded table must be written through its owning store.

## Alternatives considered

- Emitting a per-row event for each cascaded change is rejected. Those events
  would describe a device no client tracks.
- Writing the preferences or pinned apps tables directly from this module is
  rejected. The announced surfaces registry exists to refuse that.
- Renaming the parent key in place is not possible, because the foreign key has
  no `ON UPDATE CASCADE`.
- Treating `AlreadyDone` or `NoSuchDevice` as errors is rejected, because a
  client retries the call.
