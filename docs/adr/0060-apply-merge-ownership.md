# 0060: An in-flight conflict resolution owns its change's merge exclusively

- **Status**: Accepted
- **Date**: 2026-08-11

## Context

When Apply finds that `main` has diverged, the engine hands the merge to a
coding agent. There are three tiers of that hand-off: in-place in a live session
(`spawn_in_place_conflict_recovery`), detached against the thread's own worktree
(`run_merge_session_tier2`), and detached in a temp worktree
(`spawn_merge_session`). All three return `ApplyStatus::Conflict` immediately so
the caller's turn is not held open for a resolution that can take minutes.

On 2026-08-11 that gap was entered twice for the same change. A parent thread's
agent loop called `apply_change`, which spawned the Tier-2 resolution and
returned `Conflict`. Two minutes later the same loop called `apply_change`
again. By then the resolver had committed its merge but was still at step 2 of
its 5-step prompt (harden, run both suites, fix failures). The second call took
Tier 1, because the resolver's own session is a live session, its inline
`catchup_and_ff_to_main` succeeded, and `main` moved. `apply_now_success` then
ran `git reset --hard main` + `git clean -fd` inside the resolver's worktree
while it was working, and emitted a `CodingAgentIdled` for a turn that had not
ended. The resolver went on to run the test suites against code already on
`main`; a failure would have landed as a new pending change, because the
original was already resolved.

The only re-entrancy guard was `apply_now_in_progress`, an in-memory flag on
`AgentSession`. Only `apply_now` and the Tier-1 path ever set it; the detached
Tier-2 and Tier-3 spawns did not. `MergeResolutionStarted` and
`changes.merge_worktree_path` exist but are Tier-3 only. So nothing in the
system stated that a resolution was under way.

## Decision

From the moment a merge prompt is handed to a coding agent for a change until
that change's merge pairing closes, the resolution session is the **sole owner**
of the merge. Every other apply path refuses: no fast-forward of `main`, no
worktree reset, no `ChangeApplied`.

Ownership is decided by `change_ops::decide_merge_ownership`, from two terms:

1. **The durable term**: the `MergeConflictDetected` pairing. Every tier opens it
   through `start_merge_and_get_prompt`, and `ChangeApplied` /
   `ChangeApplyFailed` / `MergeResolutionCleared` close it
   (`ChangesProjection::conflict_pairing_open`).
2. **The resolver term**: a live session on the thread is bound to THIS change's
   resolution (`AgentSession::conflict_change_id`). Tier 2 and Tier 3 get the
   binding at session registration, from the `conflict_change_id` they already
   carry; Tier 1 sets it in `cc_assisted_merge_then_ff`, because it injects the
   merge prompt into a session that already existed.

A refusal is `ApplyStatus::Conflict`, never an `Err`.

## Rationale

The pairing is the only marker every tier already writes, so gating on it needs
no new emit, and it is event-sourced, which means the guard survives an engine
restart exactly as the resolution duty does.

The resolver term is what makes an exclusive lock safe to hold. A pairing left
open by a crash would otherwise refuse Apply forever, and there is no timeout
that is both short enough to unwedge a stuck change and long enough not to fire
during a genuine forty-minute resolution. Because an engine restart empties
`agent_sessions`, "nobody is carrying this pairing" is a precise, self-healing
statement, and the apply falls through to the ordinary tiers, which is the
recovery that already existed.

It has to name the resolver rather than settle for "any live session on the
thread". A stranded pairing is not closed by an ordinary follow-up (that is
deliberate: `resolve_continue_conflict_duty` binds a duty only to a
recovery-shaped continuation, since an `answered_after_idle` turn ending
`Generated` must not silently ff-merge a change nobody re-approved). So with the
coarser term, a thread carrying a stranded pairing would refuse Apply for the
length of every later turn the user ran in it, over and over, reporting a
resolution that does not exist.

The binding is descriptive, not a claim, and that distinction is the whole
reason it is safe. Nothing has to clear it: the guard also requires the pairing,
and the binding dies with the session. A Tier-1 session deliberately outlives
its own resolution, and the lingering binding there is harmless for exactly that
reason.

An unanswerable pairing query is treated as "in flight". Merging under a working
resolver is the direction that destroys something; refusing costs a retry
(`.claude/rules/rust.md`).

`Conflict` rather than `Err` because all three consumers already read it as "an
agent is resolving it": the frontend keeps its spinner, the Apply-All driver
waits for the terminal event, and the `apply_change` LLM tool echoes the typed
result. An `Err` would surface a red failure for a change that is fine.

`apply_now` is the exception that proves the point. It has no `ApplyResult` to
return, so its refusal IS an `Err`, and the HTTP layer must map it to **409**.
`404` there is not a generic failure: it is the frontend's "no live
coding-agent session" signal, and it answers by applying the thread's pending
changes one at a time. `api::claude_code::apply_now_error_status` therefore
matches the refusal by identity against the message const, so rewording the
message cannot silently reclassify it.

## Consequences

- `main` moves for a conflicted change exactly once, from the resolution
  session's own completion (`finalize_direct_agent`), after its whole turn has
  finished, tests included. That is what the user asked for: no merge before the
  merge agent is done.
- A second Apply during a resolution is now a no-op that reports honestly rather
  than a silent race. Retrying is safe: the guard sits ahead of every side
  effect, before the implementation-plan gate, the harden gate, the dirty-tree
  auto-commit and all tier dispatch.
- A resolution session that goes idle holding an open pairing (the deliberate
  `ConflictResolutionCleanupAction::HandOff` shape, waiting for a continuation)
  refuses Apply until the continuation resolves it, the user stops the thread,
  or the change is discarded. All three close the pairing. This is the accepted
  cost of the lock, and it is bounded to a session that really is carrying the
  duty.
- **Accepted gap: the Tier-2 / Tier-3 startup window.** Those tiers open the
  pairing inside the spawned task, before `run_direct_agent` registers the
  session, so for a moment no resolver is named and a concurrent apply is
  allowed through. `main` cannot move there: reaching a resolution at all means
  `catchup_and_ff_to_main` already failed on this branch, and nothing has merged
  yet in that window, so a second apply's ff fails identically and it spawns
  another merge attempt rather than a merge. Closing it would take an in-memory
  claim spanning a spawn, which is the shape rejected below. Tier 1 has no such
  window because it binds before it opens the pairing.
- Not covered: Apply arriving while an ordinary, non-merge turn is running still
  fast-forwards `main` and resets the worktree under the live agent. Same
  hazard class, different decision, deliberately left open.

## Alternatives considered

**Set `apply_now_in_progress` from the Tier-2 and Tier-3 spawns.** The smallest
diff, and it reuses the guard that already exists. Rejected because an in-memory
*claim* has to be cleared on every exit path (normal, timeout, panic, agent
death, engine restart), and a missed clear wedges Apply for that thread with no
self-heal. This codebase has been bitten by exactly that shape.

`conflict_change_id` is not that. A claim says "I hold this, until I release
it", so a lost release is a lock nobody can open. The binding says "this session
was spawned to resolve X", which stops being true when the session stops
existing, and which cannot refuse anything on its own because the guard also
requires the pairing to be open. Nothing clears it and nothing needs to.

**Gate only on the pairing, with no session term at all.** Simplest possible
rule, and it closes the startup window too. Rejected because a pairing outlives
the thing it describes: a crash or a hand-off leaves it open with nobody
carrying it, and Apply would then refuse forever with no way for the user to
clear it short of discarding the change.

**A timeout on the pairing.** Refuse only for N minutes after
`MergeConflictDetected`. Rejected because no N works: a resolution can legitimately
run longer than any timeout short enough to be useful, and the failure mode is
the incident itself, silently, at minute N.

**Serialize on `MERGE_MUTEX` for the whole resolution.** It already serializes
the git-level merge. Rejected because it is held across a coding-agent
subprocess that can run for many minutes, and every data-API write takes the
sibling `workspace_repo_lock` behind it; the existing code comments call out
precisely why no tier holds it across an agent await.

**Have the second apply wait for the resolution instead of refusing.** Rejected
because the callers are already event-driven: the Apply-All driver advances on
`ChangeApplied` / `ChangeApplyFailed`, and the LLM tool gets the typed result
back. Holding a turn open for the wait is the thing the tiers detach to avoid.
