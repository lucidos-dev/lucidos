# 0300: An engine-written thread seed is attributed to the engine; the confirming device rides beside it, never as the origin

- **Status**: Accepted
- **Date**: 2026-09-27

## Context

The engine seeds some threads after a user action. Confirming a plugin install
or update spawns a *setup thread*. Clicking **Propose upstream** spawns a thread
that offers local plugin edits to the author. Both first messages went out with
no origin and no parent link. So the message route popover read Origin
"Unknown" under a chip that read "Lucidos Engine".

The obvious fix was to stamp the clicking device. That was tried: it paired a
`Device` origin (mode Human) with the sub-thread's hardcoded Agent mode, and
`make_message_received` panicked on the mismatch. The workaround was
`origin: None`, which is what produced "Unknown".

Plan: `docs/plans/2026-09-27-engine-seeded-messages-name-their-origin.md`.

## Decision

An engine-written seed carries `MessageOrigin::Engine` with a typed reason
(`plugin_setup`, `plugin_upstream_proposal`) naming what the engine acted on.
The device that confirmed rides inside the reason as `confirmed_on_device_id`.
The seed's mode is derived from its origin, so it is `Engine`.

## Rationale

The origin answers "who wrote this". The engine wrote the seed text; the user
never typed it. A `Device` origin renders the chip as "You" and puts the
engine's words in the user's mouth. That is the same lie the "You" chip is
reserved against for API callers.

The user's click still matters: it is the authorization. It belongs in the
record, but as secondary attribution, which the popover shows as "Confirmed
on". The `ThreadQueued` audit event keeps the full actor, as before.

Deriving the mode from the origin removes the mismatch at its source rather
than steering around it.

## Consequences

- The popover names the plugin, the versions and the occasion, not "Unknown".
- Only a device fills `confirmed_on_device_id`. An API or cross-workspace
  caller leaves it empty, since the popover can only name a device.
- `ThreadQueue::submit` asserts that every spawn carries an origin or a parent
  link, so a new engine seed cannot ship unattributed without a failing test.
- Rows written before this carry nothing. The frontend names the engine for an
  agent- or engine-mode message with nothing recorded, and says the reason was
  not recorded.

## Alternatives considered

- **Stamp the clicking device as the origin, and run the thread in Human
  mode.** Rejected: it attributes engine-written text to the user, and Human
  mode changes routing (pre-emit, held messages) for a message nobody typed.
- **Pass the device as the legacy `device_id` field.** Rejected: in Agent or
  Engine mode a device id synthesizes no origin, so it changes nothing. It
  would also be a second, untyped channel for the same fact.
- **One generic `engine_seeded` reason.** Rejected: the popover then cannot say
  which plugin or which versions, which is the useful half of the answer.
- **Make `SubThread::origin` non-optional.** Rejected: a request queued before
  the field existed must still deserialize across a restart, and there is no
  honest default to fill in. The guard lives at `submit` instead.
