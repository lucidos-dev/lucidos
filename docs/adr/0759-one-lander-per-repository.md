# 0759: One designated workspace lands queued Lucidos-source changes per repository

- **Status**: Accepted
- **Date**: 2026-11-21
- **Amends**: [0463](0463-landing-lane.md), [0589](0589-github-first-landing.md),
  [0694](0694-harden-round-in-minutes.md)

## Context

Separate landing lanes sharing an origin can move main during one another's
resolution or hardening rounds. Each move can force another catch-up, even
when the incoming changes touch different paths.

## Decision

A person chooses one source workspace as the lander in Settings → Coding
Agents. Here is the default. Another host hands off the frozen Apply
snapshot using an annotated queue tag on origin. The lander takes these
entries through the same clean pass, batch and arrival hold as local Apply.

The lander publishes main and deletes the exact queue tags in one atomic
push. To return a failure or removal, it publishes a result tag and deletes
the corresponding queue tag in one push. The source recognizes a landing
on main or records the result before clearing it. The public mirror rejects
both namespaces.

Under main-check coverage, disjoint clean catch-ups carry the existing
certificate and overlapping clean catch-ups verify the overlap. Conflicting
catch-ups and catch-ups without coverage require a full round. See
[0758](0758-lane-catch-up-certificates.md).

## Rationale

Origin gives every participating checkout a shared view of pending work and
results. Tag leases let a lander prove that the advertisement it uses still
owns the queue slot. Atomic publication ties that proof to the outcome.

## Consequences

A source waits while the lander or origin is unavailable. A person can
promote another workspace explicitly. When a lane observes another lander
take a queue entry or follows main during an open batch, its menu warns
that multiple landers are active. Local-only landing retains its local path.

## Alternatives considered

Automatic promotion would turn an outage into a race between writers.
Direct gateway calls would make queue delivery depend on pairing direction
and gateway reachability.
