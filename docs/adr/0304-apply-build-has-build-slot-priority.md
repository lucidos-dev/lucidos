# 0304: The Apply rebuild is a priority waiter in the build-slot pool

- **Status**: Accepted
- **Date**: 2026-09-27

## Context

After an Apply, the popover read "Building new version" for 25 minutes. The
build itself took 2 minutes 6 seconds. It spent the rest waiting for a *build
slot*, because every slot was held by coding-agent work: an e2e release build,
lint, and harden test runs.

ADR 0070 made the pool deliberately not a queue. A freed slot goes to whoever
samples first. That is fair among agent builds, which nobody is watching. It is
wrong for the one build a user is watching. An agent test run took a freed slot
while the Apply build had already waited eight minutes. The popover also hid
all of this, since it said "Building" the whole time.

## Decision

The engine's own background rebuild waits as a **priority waiter**. While one
waits, ordinary builds leave a freed slot to it. It runs un-niced unless
`LUCIDOS_BUILD_SLOT_NICE` says otherwise. The engine reports the wait as
`build_queued` on version-status, and the UI says "New version queued".

## Rationale

**The flag is a flock, so ADR 0070's one property survives.** A priority waiter
holds a SHARED lock on `priority.lock` for exactly as long as it waits. Ordinary
acquirers probe it with a non-blocking exclusive lock. The kernel drops it on
death, so no priority claim can outlive its waiter. There is no ticket and
nothing to reclaim.

**One class, not a queue.** Arrival order among ordinary builds stays a
non-goal, for the reason ADR 0070 gives: a FIFO needs tickets, and tickets go
stale. Two classes need only one flag.

**The probe fails open.** A priority file that cannot be opened reads as "no
priority waiter". The cost is one lost turn for the Apply build. Failing closed
would stall every build on the host behind a broken file.

**Un-niced, because the user is waiting.** ADR 0210 niced every holder equally,
so nice decided only build-versus-everything-else. The Apply build is the one
the user is actively waiting for, so it should also win against the agent builds
beside it. Its core share is computed as before.

**Queued is measured from kernel facts.** A slot's holder is the
`lucidos build-slot` wrapper, which stays in the build's process group (ADR
0070). The engine publishes its build's group while the build runs. So "every
slot held, none by a process in our group" is exact, with no output parsing.

**Only the engine asks.** The engine sets `LUCIDOS_BUILD_SLOT_PRIORITY=1` on its
`web-dev.sh --engine-build` spawn, and the wrapper strips it from the tree it
admits. Nothing else in the repo sets it.

## Consequences

- An Apply build still waits for the first slot to free. No running build is
  stopped for it, so the worst case is the longest agent build in flight.
- Capacity is unchanged. Priority reorders the line; it adds no slot.
- Agent builds can wait longer while an Apply build waits. That is the intent.
- A co-located peer's build never reports queued, because we do not know its
  process group. It keeps the plain building spinner.
- `lucidos build-slot --status` names a waiting priority build.

## Alternatives considered

**An extra slot reserved for the Apply build.** Would start it at once. Rejected:
capacity is derived from RAM, and a fourth concurrent build is the OOM the pool
exists to prevent.

**Preempt an agent build.** Would start the Apply build soonest. Rejected: it
throws away minutes of agent work and fails a harden run for a reason the agent
cannot see or fix.

**A real FIFO.** Would bound every build's wait. Rejected for ADR 0070's reason:
tickets need liveness checks and reclaim, the stale state the pool was built to
avoid. It also would not have helped here, since the Apply build still sits
behind every agent build already queued.

**Parse the build's output for the wait.** The wrapper already prints "waiting"
lines. Rejected: a format contract between a shell pipeline and the engine,
where a process-group check answers the same question exactly.
