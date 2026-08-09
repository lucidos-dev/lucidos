# 0057: A blocked e2e run subscribes to the lock's release event; the cross-workspace gap recovers on the timeout

- **Status**: Accepted
- **Date**: 2026-08-09

## Context

The *e2e lock* is machine-wide: one file, one path, contended by every workspace
on the host. It exists because two concurrent Playwright sessions OOM-rebooted a
32 GB Mac on 2026-04-19, and it is not negotiable. What was never designed is how
the loser waits.

On 2026-08-09 three coding-agent threads raced for it at once. One held it mid
mobile-webkit run. The second wrote `/tmp/run-e2e-retry-<pid>.sh` containing
`for i in $(seq 1 120)` around `./scripts/e2e-browser.sh` with `sleep 20;
continue` on refusal: a 40 minute foreground tool call that re-executed the entry
script's build checks every 20 seconds. The third parked on a bare `sleep 20`.
Each waiter burned a Claude Code turn and held engine capacity for the whole
wait, to learn something the engine already knew.

Lucidos owns the right primitive already. A thread can register an *event wait*
(`lucidos await-event`), end its turn, and be re-opened when a matching event
lands. Nothing emitted a lock event, so nothing could be waited on.

## Decision

The lock announces every hold as a domain event: `E2ELockAcquired` when a run
takes it, `E2ELockReleased` when a hold ends, emitted best effort from
`scripts/lib/e2e_lock.sh`. A refused run subscribes to `E2ELockReleased` and ends
its turn.

The **cross-workspace gap is accepted, not closed**. `lucidos events emit` writes
to the emitting subprocess's own `$LUCIDOS_WORKSPACE`, so a holder in workspace A
releasing does not wake a waiter in workspace B. The subscriber's own
`--timeout-secs` deadline is the recovery, and the gap is stated in the refusal
message, in the skill and in the harness doc rather than left to be discovered.

## Rationale

**Polling was never a waiting strategy, it was the absence of one.** Its cost is
not the 20 second granularity, it is that a turn is held open for hours, engine
capacity with it, and the entry script's build checks are re-run on every
attempt. A subscription costs one idle thread and one event.

**Both endings of a hold emit, including the reclaim.** A waiter is blocked on
the hold, not on the process. A holder killed hard enough to skip its EXIT trap
never announces anything, so the run that later reclaims the stale lock emits the
release on its behalf, describing the dead owner rather than itself. Without that
arm, the most common abnormal ending strands every waiter until its own deadline,
which is precisely the case a subscription is supposed to improve on.

**Best effort is a requirement, not a compromise.** An e2e run must not go red,
and an EXIT trap must not stall, because the engine was briefly unreachable. That
makes the emit lossy by construction, which is what forces the timeout to be a
real recovery path rather than a formality.

**Closing the cross-workspace gap costs more than it buys.** The two ways to
close it are both worse than the timeout (see below), and the gap is narrow in
practice: the common contention is several coding-agent threads inside one
workspace, exactly the case that already works. What made the gap unacceptable
was not its existence but its *silence*, and silence is cheap to fix: the refusal
compares the holder's `WORKTREE` against `$LUCIDOS_WORKSPACE` and says so when it
can tell, staying quiet rather than guessing when there is nothing to compare
against.

## Consequences

- A blocked session ends its turn. Engine capacity is freed while it waits, and
  the entry script runs once per genuine attempt rather than once per tick.
- The refusal message is now load-bearing documentation. An agent that never
  loaded the skill still gets the subscribe path, so it must stay short and
  correct.
- Two paths do not wake a waiter, and both recover on the deadline: a holder in
  another workspace, and a release emitted while the engine is down. A waiter
  must therefore treat the timeout as a case it handles by reporting, not as an
  error.
- **Both announcements of one hold are addressed as of the moment it was taken,
  and that had to be made true** (`_e2e_capture_emit_env`, added 2026-08-09).
  "The emitting subprocess's own `$LUCIDOS_WORKSPACE`" is not one workspace per
  run: `acquire_e2e_lock` runs first, while the variable still names the
  caller's, and then `reset_e2e_database` reaches `setup_postgres`, which
  exports the e2e-test workspace into the entry script's **own** shell. The
  release, emitted from the EXIT trap after that and after
  `stop_e2e_workspace`, went to an engine teardown had just stopped, in a
  workspace no waiter watches. It read as this decision's accepted gap and was
  not: a waiter in the SAME workspace as the holder was never woken either, so
  the case named above as "exactly the case that already works" did not. One
  workspace's event store held 20 `E2ELockAcquired` against 3 `E2ELockReleased`
  on the day this was found, the three being the only paths that end before
  `setup_postgres`. The emit environment is now captured once at acquire and
  applied through `env`. The cross-workspace gap is unchanged by that fix and
  stays accepted.
- The lock file gains a `STARTED_EPOCH` key so `held_secs` is portable
  arithmetic. Readers ignore unknown keys, so a lock file written before it
  existed still reclaims; its release just carries no `held_secs`.
- The emit is suppressed while `$E2E_LOCK_DIR_OVERRIDE` is set. That variable is
  the lock library's test sandbox, so the guard also means the library's own
  suite can never write events into a developer's live workspace.
- Waiters race. Several wake on one release and exactly one wins, so a refused
  retry is the expected case and re-subscribing is normal. The engine's cap of 10
  consecutive subscriptions with no user message is what bounds that loop, which
  is why the skill also caps attempts and hands the decision back to the user.

## Alternatives considered

**Keep polling, but make it cheaper** (a short-circuit flag that checks the lock
without re-running the build checks). Rejected: it addresses the least important
cost. The turn is still held open for hours and the engine capacity is still
occupied, which is what actually hurt on 2026-08-09.

**A second emit into the holder's peer workspaces.** Rejected. The emitter would
have to enumerate every workspace on the machine, resolve each engine's port, and
POST to it, which is hand-rolled HTTP at another engine's API, from a shell
script, on a teardown path that must never fail or stall. It also inverts the
ownership: a workspace's event store would carry events authored by a process
belonging to another workspace, with no actor that makes sense.

**Have the waiter watch the lock file instead of an event.** Rejected: that is
polling with extra steps, and it cannot end the turn, which is the entire point.
An `fswatch`-style wait would end the turn only by holding a process open, which
dies with the session.

**A trigger instead of a subscription.** Rejected: a trigger is a standing rule
that spawns a NEW thread on every match and outlives the conversation. A waiter
wants exactly one wake on an existing thread, which is what an event wait is.

**Emit only on release, not on acquire.** Weighed and rejected on balance. The
acquire event costs one extra best-effort POST per run and makes the timeline a
sequence of paired intervals: who holds the lock, since when, and whether they
took it over from a dead owner. With releases alone, every hold reads as an
unpaired half.

**Make the emit reliable** (retry, or a spool file replayed by the next run).
Rejected: it trades the one property that must hold, that a teardown never stalls
and never reds a green run, for a gap the timeout already covers.
