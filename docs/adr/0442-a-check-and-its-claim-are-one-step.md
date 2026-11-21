# 0442: A spawn claims its thread, and an apply its change, in the same step that finds it free

- **Status**: Accepted, amended by [0463](0463-landing-lane.md)
- **Date**: 2026-10-12
- **Amends**: [0026](0026-a-session-is-never-owned-by-a-request-future.md),
  [0060](0060-apply-merge-ownership.md),
  [0249](0249-a-sub-thread-does-not-park-its-parents-change.md),
  [0311](0311-worktree-targets-shrink-and-release.md)

## Context

`run_direct_agent` checked for a live session under the `agent_sessions` lock,
dropped the lock, and inserted its own entry about 600 lines later. Worktree setup,
git calls and the two-permit startup semaphore sat in between. Each Continue click
emits its own `ContinuationRequested`, and the spawn dispatcher dedupes by event id
only. So two clicks more than three seconds apart both passed the check and ran two
agents in one worktree. A Stop in the same window found no entry and landed on
nothing: the agent started anyway.

The window was covered by patches rather than closed:

- `SPAWN_DEBOUNCE`, a three-second per-thread stamp with its own error;
- `SpawnsInFlight`, a counter no spawn read, which also counted its own run;
- `SettleUnlessOwned`, which mapped a lost debounce to a conditional settle;
- `wait_for_cc_session_alive`, a 30-second poll for a chat follow-up that arrived
  while the chat leader was still starting;
- `cc_spawn_coalesce`, a leader election with a 250 ms sleep for messages racing
  one chat spawn.

Recovery spawns (hardening, the Tier-2 merge, a re-attached conflict duty) skipped
the check entirely, so they could insert over a live session.

Apply had the same shape. `apply_change` read the change row and checked
`is_pending()` with no claim, on the HTTP, Apply All, standing-apply and LLM tool
paths. Two Tier-3 applies both reset the shared `apply-<change_id>` temp worktree,
and the duplicate `ChangeApplied` was filtered at the bus afterwards
(`change_apply_dedup`). ADR 0060 accepted the Tier-2 / Tier-3 startup window.

## Decision

**A spawn claims its thread in the lock hold that finds it free.** The claim inserts
the thread's `AgentSession` in `SessionPhase::Starting`, and `run_direct_agent` takes
the claim by value, so no spawn runs without one (`agent_session/spawn_claim.rs`).

- The entry owns the message channel, the stop and interrupt signals and the
  cancel tokens from the start. A follow-up sent during startup queues for the run.
- A Stop on a starting entry cancels the spawn (`startup_cancel`). The agent never
  takes its first turn, and the turn ends `ResponseCanceled{UserStop}`. A Stop that
  lands after the process spawned kills it, and the claim is held until it is gone.
  Apply, Discard and Archive are recorded as `pending_stop`, as on a running
  session, and the loop carries them out as it starts.
- A caller's stale-resume retry runs on the same claim. The stale run puts the
  entry back to `Starting`, so nothing takes the thread between the two runs.
- The spawn upgrades the entry to `SessionPhase::Running` in place, in the lock
  hold that re-checks for a Stop. It never inserts a second entry.
- **A thread stays held until its run lets go of the receiver**, not only until it
  marks its process exited. `finalize_direct_agent` still commits, proposes or
  discards in the worktree after that mark. A spawn that finds a finishing run
  waits on `msg_tx.closed()`, which resolves when the run lets go.
- Every spawn path claims, recovery spawns included. A claim never replaces a live
  run. The external watchdog removes a wedged run's entry before it requests the
  continuation, so recovery has nothing live to replace.
- The chat lane claims at its fast-path check, under the lock the follow-up checks.
  A follow-up either finds the run that holds the thread and routes to it, or
  claims the thread itself.
- A settle of a `running` projection that no run holds takes the claim for its
  length (`settle_unclaimed_running_thread`), so no spawn starts under it.

**An apply claims its change before it reads the row** (`change_ops::apply_claim`).

- `apply_change` claims first. A held claim refuses with `Conflict` and no failure
  event: the apply that holds it announces the outcome.
- The claim travels with the work. The Tier-1 recovery task, the detached Tier-2
  task and the Tier-3 merge session hold it until they finish. `run_direct_agent`
  borrows it as `conflict`, so the conflict-resolution completion emits under it.
- A merge session that hands its duty to a continuation (the watchdog's, or the
  safety net's) releases the claim to it. A wedged run may never return, so the
  claim records the session's hand-off flag, and a later claim takes over.
- `emit_change_applied` takes `&ApplyClaim`, and it is the only place that builds
  `ThreadEvent::ChangeApplied`. Paths that find a change by an earlier read
  (`apply_now`, the external-repo archive, a re-attached duty) claim it, then read
  it again (`claim_pending_change`).
- Discard and set aside take the same claim. An apply cannot merge work the user
  has just discarded, and a discard cannot reset a tree an apply is merging.
- The claim is in memory. One engine owns the database (`startup_lease.rs`), and a
  restart releases every claim. A crash between moving `main` and the event is
  healed by the next apply, as before.
- The startup lease can fail open, and then two engines share the database with
  separate claims. A unique index, `events_change_applied_once`, refuses a second
  `ChangeApplied` for one change at the database. It does not stop the second
  engine's merge; closing that is the lease's job.

**A Tier-2 apply holds its thread** with the same claim, in `SessionPhase::Applying`,
from before the pre-merge commit until the merge is done with the worktree.

- A spawn or a wake that finds the hold waits on two signals: the apply letting
  go, or the apply handing the thread over. Then it claims again.
- The apply's own merge session takes the hold over (`hand_apply_hold_to_run`),
  which flips the entry to `Starting` in one lock hold. A waiting follow-up then
  finds a live run and routes to it.
- A Stop or Discard on a held thread is refused out loud: no loop listens to the
  entry, so a recorded stop would be lost.
- An apply that finds a spawn starting refuses with `Conflict`, rather than reset
  the worktree under it.

The patches above are deleted, and so is the bus-level `change_apply_dedup`.
`StaleSettle` and `settle_stuck_running_thread` stay: they cover a crash, a
restart and the boot sweep.

## Rationale

The bad interleaving was a check and an act in two steps. Making them one step
removes the interleaving instead of shrinking it, and every patch shrank it.

The entry is the right carrier because every reader already asks the map. Stop,
the follow-up fast path, worktree cleanup and the watchdog needed no second
registry once a starting run was in the map.

Holding the thread until the receiver drops costs nothing new: the receiver
already decided liveness (ADR 0026). It closes the second, smaller window, where a
follow-up spawned into a worktree that finalize was still writing.

## Consequences

- An entry no longer implies a running process. It implies a claimed run,
  starting or running, or an apply holding the thread. Readers that act on the process ask the phase:
  `deliver_control_request` refuses a starting agent, the external watchdog skips
  it, and `arm_followup_redirect` never interrupts it.
- Messages that race one chat spawn are no longer combined into its first prompt.
  The second queues for the run and is read after the first, like any follow-up.
- A follow-up that queued for a run that then failed to start goes to the orphan
  chain when the claim drops, so it is answered.
- A spawn can wait for a finishing run. Finalize is bounded by git work, so the
  wait is seconds.

## Alternatives considered

**Keep the debounce and lengthen it.** No interval works. A slow startup outlasts
any short one, and a long one refuses legitimate resends.

**Claim inside `run_direct_agent` only.** It closes the double spawn but not the
chat follow-up window: the chat leader takes `active_threads`, emits, and generates
titles before it reaches the spawn. The poll would have to stay.

**A conditional `UPDATE changes SET status = 'applying'`.** The `changes` table is
a projection: only `changes_projection.rs` writes it, inside the event
transaction. A direct write could not be replayed by `rebuild_missing_from_events`,
and every reader that filters on `pending` would need to learn a new state. A
crashed engine would also leave rows `applying` for a boot sweep to release.

**Keep the bus filter as defence in depth.** It hid the race rather than closing
it. A read and a suppress in the emit transaction is a filter after the fact. The
unique index states the same rule as a constraint, and holds across engines.

**A separate starting registry beside the map.** `SpawnsInFlight` was that. Two
registries need two locks and every reader must ask both, which is how the counter
ended up read by nobody who spawns.
