# 0743: Exclude build-slot queue time from background task run budgets

- **Status**: Accepted
- **Date**: 2026-11-19
- **Builds on**: [0070](0070-engine-owned-build-slot.md), [0047](0047-event-wait-is-an-event.md)

## Context

A background task starts its watchdog when it spawns. A build broker inside the
task can spend most of that budget waiting for a slot.

## Decision

Give the task a private Unix socket. A broker holds a connection while queued.
The watchdog pauses its run clock while any broker is queued and enforces an
absolute ceiling. Closing the connection also settles a queue that ended without
a grant.

The engine records timing changes on the original wait ID. Replay applies the
latest timing without changing the wait's original subscription or watermark.
The row reads typed queue state and the watchdog deadline.
The subscription keeps a separate expiry for completion delivery.

## Consequences

The watchdog works on the same task-local clock for polled and watched work.
Only engine-owned background waits use timing updates. A broker outside a
background task continues to use its own slot queue.
