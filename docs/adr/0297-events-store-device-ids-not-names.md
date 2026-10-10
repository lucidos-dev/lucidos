# 0297: Events store a device's id, never its name; displays resolve the name

- **Status**: Accepted
- **Date**: 2026-09-27

## Context

A device actor was stored as `MessageOrigin::Device { device_id, label }`, and
`MessageReceived` also carried a `device` name. Both were copies of the name
the device had when the event was written. A message's Origin popover showed
`device-ab2c03f7` for a phone the Devices page called "Safari on iPhone". The
copy was taken before the engine knew the pairing label, and a copy never
learns anything later: not a rename, not a label that arrives afterwards.

## Decision

An event stores the device's id and nothing else. Every surface resolves the
name from the id at display time, through one function per layer:
`friendly_device_name` / `DeviceStore::friendly_name` in the engine and
`deviceFriendlyName` in the app.

## Rationale

The `devices` table owns the name. A second copy in each event is duplicated
state, and it goes stale on the first rename. Storing the id and resolving the
name is what makes a rename reach every older event, the agent's context and
every screen at once. Events stay immutable: nothing rewrites them, and old
rows keep their stored names, which nothing reads.

## Consequences

- The app loads two lists at startup, the engine's devices and the gateway's
  pairings, since every screen that names a device reads both. The name slot
  is empty until they land. The Devices page reads the same two, through the
  same `deviceName`, so the two surfaces cannot disagree.
- A device that was Removed has no row, so it shows `device-<first 8>`.
- A workspace app reading `actor.label` from an event now gets nothing. No SDK
  call lists devices, so an app keeps the id and loses the name.
- `api::actor::user_actor` needs no database: it builds the actor from the id.

## Alternatives considered

- **Keep the stored name and resolve live on top of it.** The previous change
  did this: the popover looked the name up by id and fell back to the copy.
  It left two sources for one fact, and the copy was still what the agent
  context and apps read.
- **Rewrite old events with the current name.** Events are immutable, and the
  next rename would make the rewrite stale again.
- **Resolve the name in the events API before serving it.** It keeps `label`
  for apps. But it re-derives a copy on every read, and it puts a name in the
  wire shape that is not part of the event.
