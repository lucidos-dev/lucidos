# 0817: Record card openings for each ancestor and deliver them through addressed thread waits

- **Status**: Accepted
- **Date**: 2026-11-26

## Context

An orchestrator needs to discover questions and permission requests at any
depth, including in sub-threads spawned after a wait arms. A list of source
thread IDs goes stale on each spawn. Rebuilding it costs an extra turn and
requires each intermediate agent to relay openings itself.

## Decision

When an open card posts, append one `SubThreadCardOpened` system event per
ancestor in the same transaction. Follow the persisted same-workspace edges
as of the opening, serialized with detach (ADR 0278). The recipient is the
ancestor whose thread owns the subscription. Its condition can narrow the
match. Triggers cannot watch this event.

Card admissions share a transaction lock. A move to top level or family
delete takes it exclusively before the operation locks any summary rows.
Family deletion removes relays that reference deleted threads.

The relay carries the ancestor, direct child on the path, source thread,
source event and card kind. The source card carries its own content and
continues to use the existing owner notification and answer paths.

## Rationale

Committed relay rows give live, late and restarted dispatch the same target
and payload. Each recipient scans its own aggregate above the wait watermark
(ADRs 0047 and 0364). An intermediate thread's wake mode and subscriptions
have no bearing on other recipients. An idle ancestor without a matching
wait incurs no model turn.

## Consequences

Storage grows by one row per ancestor per opening. A move to top level
changes subsequent openings and pending-card reports. A previously committed
opening remains eligible for forward delivery to its original recipients.

At arming, the result reports matching pending cards in the current tree,
including cards opened before the feature shipped. The report uses the
recipient's relay sequence when present and the source sequence otherwise.
It leaves the wait available for a future opening. A continuation uses the
saved watermark and conditions while reading which cards are still open.

## Alternatives considered

An ancestor list on the original card event would enlarge each raw card
payload and require a membership matcher. A live topology join would give
different authority for the same opening before and after a detach. A relay
aggregate persists the address and keeps the original card subscription
contract independent of tree discovery.
