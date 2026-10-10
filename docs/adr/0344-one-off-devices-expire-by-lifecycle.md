# 0344: One-off devices expire by lifecycle, never by user agent

- **Status**: Accepted
- **Date**: 2026-10-02

## Context

Every headless browser a coding agent points at a live workspace registered a
new device, and nothing removed it. One workspace held 568 such rows out of 747.
They cluttered Settings → Devices and competed for the agent prompt's
known-devices lines.

The mechanism: an agent browser authenticates to the gateway with the
machine-local token, so the gateway names no device. The frontend then mints a
localStorage id, and a fresh browser profile is a fresh device. Most of these
runs emulate a phone or a desktop browser, so only a few say `HeadlessChrome`.

## Decision

The engine removes every *one-off device* in its daily device sweep. A one-off
device was created over a week ago and never seen 24 hours or more after it was
created. It also has no typed name, no pairing label, and no push state.
`DeviceStore::remove_one_off` holds the rule.

## Rationale

The rule keys on what the throwaway rows have in common, not on what they claim
to be. A test profile is used for one session and then discarded. That holds
whatever browser it emulates and however it authenticates.

The exclusions keep every device someone deliberately set up. A name, a pairing
and push are each a user decision. A real device used again within a week is
seen again, and it then never qualifies.

Removal is cheap to undo. The browser keeps its id, so a device that does come
back registers again as itself. It loses only device-scoped preferences and
pinned apps.

## Consequences

- Throwaway rows live at most about a week, then go with one `DeviceDeleted`
  each.
- A real device used once, then again after more than a week, loses its
  device-scoped preferences and pinned apps.
- The agent prompt's known-devices list is unchanged. A throwaway that is
  visible right now still ranks by recency, which is true while it runs.
- Deleting a device now also clears its presence row, on both the sweep and the
  Remove button.
- The sweep removes one-off devices before the 30-day push prune runs, so it
  never acts on a push flag it turned off itself. A device whose push the
  prune turned off, and which has no push subscription left, can qualify on a
  later day. Its push endpoint is already gone, so nothing could reach it.

## Alternatives considered

- **Filter on the `HeadlessChrome` user agent.** Rejected: emulated runs claim
  to be iPhones and desktop Chrome, so it caught 2 of 139 rows. It would also
  punish a person who really browses headless.
- **Tag registrations that arrive with the machine-local token.** The gateway
  knows the caller is a local process, so it could forward that to the engine.
  Rejected: it needs a new trusted header, a schema column and a migration. The
  lifecycle rule already catches these rows without it, and the tag would miss
  a throwaway that reaches the engine another way.
- **Hide one-off devices from Settings and the prompt, keep the rows.**
  Rejected: a display filter leaves the table growing, which is the symptom
  this decision set out to remove.
- **Stop agent browsers registering at all.** Rejected: chat attribution and
  device-scoped preferences need a registered device, so an agent testing those
  paths would break.
