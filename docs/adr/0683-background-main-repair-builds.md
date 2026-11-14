# 0683: Automatic main repairs use a yielding background build class

- **Status**: Accepted
- **Date**: 2026-11-13
- **Amends**: [0304](0304-apply-build-has-build-slot-priority.md), [0210](0210-build-slot-governs-cpu-share.md)

## Context

An admitted automatic main repair must not take a freed build slot ahead of chosen work.
Running builds remain non-preemptive.

## Decision

Build slots have ordinary, priority and background classes.
Background waiters yield while any ordinary or priority build waits.
Chosen work keeps the existing waiter flag; background waiters use a separate flag.
Background attempts hold the existing flag exclusively across the slot walk.
They therefore yield to older brokers too, without waiting on their own flag.
The existing priority gate still makes priority waiters beat both lower classes.

`LUCIDOS_BUILD_SLOT_CLASS` selects the class.
The engine's Apply rebuild sets `priority`; automatic repair lineages set `background`.
Unspecified or unknown values remain ordinary.
When the new class is unset, the older engine's priority input remains a compatibility alias.
Nice defaults are 10 for ordinary, 0 for priority and 15 for background.

A persisted `MainCheckRepairStarted` with `started_by = auto` controls the repair class.
Every resume, successor, child and engine-run agent background task resolves that lineage before spawning.
Fix now selects ordinary work.

## Rationale

Separate waiter flags preserve the current pool's flock lifetime and atomic admission.
Kernel release on process death prevents stale priority claims.
Durable repair provenance keeps resumes from silently upgrading their priority.

## Consequences

Source builds on macOS and Linux share the same pool semantics.
Packaged installs do not compile Lucidos.
Slot capacity, release policy, hardening and E2E suite selection remain unchanged.
No running process is stopped when chosen work arrives.

## Alternatives considered

- Lower nice alone changes CPU contention but cannot choose who gets a freed slot.
- Killing a running repair build discards paid progress.
- A general ticket queue adds persistent ordering the pool does not need.
