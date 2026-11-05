# 0480: The landing lane tries every arrival on main first and gathers while an Apply All press is open, so a press's conflicts resolve in one lane batch

- **Status**: Accepted
- **Date**: 2026-10-14
- **Amends**: [0463](0463-landing-lane.md), [0314](0314-apply-all-parks-a-conflicting-change.md)

## Context

An Apply All press of 9 Lucidos-source changes split into two lane batches.
The first conflicting member opened a batch on its own. Six more members
waited, because their files overlapped that batch. Each batch got its own lane
thread and its own `/harden`, so the press hardened twice.

The events showed where the time went. Batch 1's lane thread spent 2 minutes
merging and 27 in `/harden`, for about $8. When it landed, 5 of the 6 waiting
members merged cleanly at once. They had waited 30 minutes for nothing.

Two rules of ADR 0463 caused this:

- Apply All hands the lane one member at a time, and a pass that sets a member
  aside opens a batch at once.
- An arrival whose files overlap the open batch waits, whether or not it merges
  cleanly onto `main`.

The measurements are in `docs/plans/2026-10-14-apply-all-press-lands-as-one-batch.md`.

## Decision

**Every arrival tries `main` first.** The footprint wait is gone. While a
batch resolves, an arrival still gets a clean pass, and lands at once when it
merges cleanly. The batch meets it later, in its catch-up merge, and a
catch-up conflict is a turn for the lane thread, as ADR 0463 already says.
What conflicts waits for the next batch.

**The lane gathers while an Apply All press is open.** Apply All tells the
lane when a press opens (`PressOpened`) and when it has nothing left to submit
(`PressSubmitted`). While any press is open and no batch is open:

- a pass's set-aside members go back to the waiting queue, and open no batch;
- the waiting queue starts no pass.

When the last open press is submitted, one pass runs over the queue, and what
conflicts opens one batch.

**A press is submitted when no member is `Queued` or `Applying`.** That is the
moment Apply All can start nothing more. Cancel and recovery count too. The
lane ignores an unknown or repeated press.

**`Laned` comes from the lane hand-off, for every caller.** A member that
hardens first is queued later by the auto-apply after hardening. Only Apply
All's member task used to turn a `Queued` answer into `Laned`, so that member
stayed `Applying` until it landed. With gathering that is a deadlock: the
press waits for the member, and the lane waits for the press. Now
`submit_to_landing_lane` sends `Laned` whenever the lane answers `Queued`.

## Rationale

- **The test that matters is the merge, not the file list.** Overlapping files
  are a guess at a conflict. `git merge-tree` is the answer, and the lane
  already asks it on every pass.
- **One resolution per press.** A press queues its members within seconds when
  none needs hardening. Holding back the batch for those seconds puts every
  conflict in one lane thread with one `/harden`. Before the lane, 23 of 112
  presses in 45 days had two or more conflicting changes.
- **Apply All stays the only orchestrator.** It still applies one member at a
  time with every gate. The lane learns only that a press is still feeding it,
  and never reads Apply All's state.
- **No table, no timer.** A gathered member is an ordinary waiting member,
  announced by `ChangeQueued`. A restart restores it as waiting and runs a
  pass, which opens a batch with whatever is there. A press a restart cut can
  still split, which is the old behaviour and loses nothing.

## Consequences

- A change that merges cleanly lands in seconds, whatever batch is open.
- A clean arrival that overlaps the batch can make its catch-up conflict. That
  costs the lane thread a turn, never a failed batch.
- A press that hardens a member holds its batch back for that harden, since
  members still harden one at a time (ADR 0314). Gathering inherits Apply All's
  own progress: every harden ends in an apply or a reported failure, and
  Cancel submits the press.
- The lane row says "Waiting for Apply All to queue the rest" while it gathers.
  The snapshot carries it as `gathering`.
- `footprint.rs` and the `Overlap` and `Arrival` types are gone.

### Amendments

- **ADR 0463, "What would conflict forms one lane batch".** An arrival no
  longer waits because its files overlap. It always gets a clean pass. The
  rationale bullet "Exact footprints make pass-through safe" is retired with it.
- **ADR 0463, "Drain every Apply All member into one pass".** Still rejected.
  Gathering gets the same single batch while Apply All keeps driving the press.
- **ADR 0314.** `Laned` reaches a press from the lane hand-off, whoever made
  it, not only from Apply All's own member task.

## Alternatives considered

- **Join an open batch.** A change set aside while a batch resolves would join
  it, with a follow-up turn on the lane thread. Measured, it saves almost
  nothing: the joiner forces a `/harden` rerun, which costs what a new batch
  costs, and delays the batch's existing members by that rerun.
- **Drain the whole press into one pass.** Apply All would run every member's
  gates and then submit them together. Clean members would then wait for the
  slowest member's gates, and ADR 0463's two-orchestrator objection still
  holds.
- **Gather for a fixed time.** A timer guesses how long a press takes. A press
  that hardens outlasts any guess, and a short press wastes it. The press
  itself knows when it is done.
- **Order or resolve by age.** Telling the lane thread which change is newer
  was considered and declined. A newer change was not written knowing the
  older one, so recency says little about which intent is right.
