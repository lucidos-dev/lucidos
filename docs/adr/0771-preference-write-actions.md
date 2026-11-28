# 0771: A preference write applies locally before the network call returns

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0437](0437-optimistic-ui-state-follows-server-acks.md)

## Context

`savePreference` applies a value before the network call. It runs the side
effect and patches the optimistic signal first, so the UI is never gated on the
round trip.

That leaves delivery as the only thing that can go wrong. On an installed iOS
PWA it goes wrong constantly. WebKit suspends the page tens of times a day on a
busy workspace and aborts every in-flight fetch.

The old code toasted that as "Failed to save <key> preference: request
cancelled". It never retried, and it left the device showing a value the server
never received.

This record restates the design comments of
`crates/lucidos-app/src/store/actions/preferences.ts`, written before the
comment strip removes them. Nobody checked it line by line against the code.

## Decision

- A write the engine has not accepted yet is kept as a pending preference
  write. Pending writes live in a map keyed by preference key.
- Each pending write carries `seq`, its position in the global request order.
  This spots a write that a newer value for the same key has superseded.
- A re-send is owed until the engine answers about that key. Both answers clear
  it: accepted and refused.
- A counter tracks consecutive writes that got no answer. Below the threshold
  the failure is silent. At the threshold it speaks once.
- Any answer resets the counter, a rejection included.
- Any answer also retracts the unreachable banner, whichever way the queue
  drained.

## Rationale

`PUT /preferences?key=<k>` is a per-key overwrite. So the pending map is
last-write-wins and not a queue. A value for a key the user has since changed
again is garbage, not backlog.

The value is already applied locally and a re-send is owed. One suspended fetch
is therefore noise rather than news, so it stays silent below the threshold.

A genuinely unreachable engine must not be swallowed, so the threshold speaks
once.

A 4xx proves the engine is reachable, so a rejection counts as an answer. The
banner must be retracted either way. Otherwise it keeps insisting nothing is
getting through while the rejection card next to it says otherwise.

## Consequences

- A write cut off by a page suspension no longer toasts an error. It stays owed
  until the engine answers.
- Only the newest value per key is ever delivered. Superseded values are
  dropped without being sent.
- A rejected write and an accepted write both stop the re-send and both retract
  the unreachable banner.
- A real outage still surfaces, once, when consecutive unanswered writes reach
  the threshold.

## Alternatives considered

- Toasting every aborted fetch as a save failure and never retrying. This was
  the old behaviour. It reported a page suspension as an error and left the
  device out of step with the server.
- A queue of pending writes. Rejected because the endpoint is a per-key
  overwrite, so older values for a key are garbage.
