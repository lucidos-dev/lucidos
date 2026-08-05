# 0045: The engine, not a client timer, decides when a switch-interrupted thread gets its Continue button back

- **Status**: Accepted
- **Date**: 2026-08-05

## Context

A user-initiated *Switch to new version* pre-emits a teardown
`ResponseAborted { cause: EngineShutdown, actor: <device> }` on every in-flight
thread, and the next boot auto-resumes those threads (see *cause-gated resume* in
`docs/glossary.md`). The frontend rendered its **Continue** button on the newest
unresumed abort with no notion of that promise, so the button sat there through
teardown, restart and boot. On 2026-08-05 a user clicked it nine seconds after the
teardown and the timeline recorded a human-attributed "Continued the response"
where the engine's own "Resumed after engine restart" belonged.

Withholding the button while the resume is pending is the easy half: the frontend
can mirror `SWITCH_TEARDOWN_ABORT_SQL`, the very fingerprint both backend resume
gates key on. The hard half is the user's actual requirement, stated as "Continue
should only be an option if auto resume doesn't work": **what brings the button
back when the promised resume never arrives?**

That case is real, not theoretical. The chat half caps resumes per boot
(`MAX_CHAT_SWITCH_RESUMES`), `continue_chat` can error, a `ContinuationRequested`
can fail to persist, the candidate scan can fail on a DB error,
`recover_orphaned_worktrees` skips branches for several reasons, and an archived
thread is selected by neither resume drain.

## Decision

The **engine** discharges its own promise. `settle_unresumed_switch_threads`
(`agent_recovery/recovery.rs`) runs last in the boot sequence, after both resume
drains, and emits a fresh `ResponseAborted { cause: RecoveryAfterRestart }` for
every thread still holding an unkept promise. That boundary is not a switch abort,
so the frontend's newest-abort scan re-arms Continue with no extra rule.

There is **no wall-clock heuristic in the browser**.

## Rationale

A client-side grace timer ("the engine has been back for N seconds and still
hasn't resumed, so show the button") is the obvious quick fix and it is wrong for
a concrete reason: a switch that triggers a dev rebuild can leave the engine down
for **minutes**. Any timer short enough to be useful re-shows the button while the
resume is still legitimately pending, which is the exact race this change exists
to close, just moved later. Any timer long enough to be safe is no longer useful.

The engine has the fact the client is trying to guess. It knows, per thread,
whether it actuated a resume this boot. Turning that fact into an event is both
smaller (one sweep, ~40 lines) and exact.

Emitting the *existing* crash-shaped boundary rather than inventing a new event or
a new status keeps the frontend rule to one line: the newest abort decides. The
boundary is also honest to the user, since the turn genuinely was interrupted and
genuinely is not resuming, and `chat::recovery::recover_orphaned_threads` already
uses it to say exactly that.

## Consequences

- The Continue button is absent for the whole teardown / restart / boot window on
  a switch, however long the rebuild takes, and returns the moment the engine says
  the resume is not coming. It never races the engine's own recovery.
- Both resume drains (`resume_pending_switches`,
  `resume_pending_chat_switches`) now **return the thread ids they actuated**, and
  the floor excludes by id rather than by query. This is load-bearing: a
  coding-agent resume has only emitted `ContinuationRequested` when the floor
  runs, and that type is deliberately absent from `THREAD_START_EVENTS_SQL`, so a
  query-only exclusion would re-abort a thread that is resuming perfectly well.
- In the failure case the timeline shows two boundaries in a row, "You Restarted"
  then "System Response interrupted". That is accepted: it only happens when the
  promise was actually broken, and the second panel is the honest record of it.
- The sweep must be idempotent across boots, since nothing supersedes the original
  switch abort in the start-event sense. It is, because its own withdrawal becomes
  the thread's newest `ResponseAborted` and the query requires the switch abort to
  be the newest one.
- A third consumer now reads the switch fingerprint, in TypeScript
  (`abortPromisesAutoResume`). It cannot import the Rust constant, so
  `switch_teardown_fingerprint_is_stable_for_the_frontend_mirror` is the canary
  that fails if either half of the pair moves.

## Alternatives considered

**A grace timer in the client.** Rejected above: a rebuild-triggering switch makes
every choice of N wrong in one direction or the other. It also puts a guess about
backend behaviour in the browser, where it silently rots as the boot sequence
changes.

**Key the button on `CodingAgentIdled { reason: engine_restart_interrupt }`.**
That is already the engine's explicit "I am not resuming this" signal, so the
frontend could re-arm on it. Rejected as the *primary* mechanism because it is
coding-agent-only: chat and trigger threads have no equivalent, and the chat resume
cap is one of the concrete gaps. Left unused rather than carried alongside the
floor, since the floor covers the same case uniformly and a second re-arm rule
would be a second thing to keep in sync.

**A second thread status (`resuming` vs `paused`), or a `resume_pending` boolean
column on `thread_summaries`.** Either would let the frontend gate the button on a
pure status read with no event archaeology. Rejected as more surface for the same
answer: a status touches the generated lifecycle contract, its cross-validation
fixture, and every exhaustive match and `Record<>` map on both sides, and a boolean
column needs a migration plus its own projection wiring. The abort event already
carries the fingerprint, and the floor already has to exist for the failure case
either way.

**Leave the thread `running` through the switch instead of settling it.** Tempting,
because `AbortCause::is_transient()` already says the turn is expected to come
back, and it would have needed no new status at all. Rejected because it
contradicts CLAUDE.md § Engine Statelessness: a `running` thread with no live
process is precisely the zombie `settle_orphaned_running_coding_agent_threads`
exists to kill, and if the new binary never boots the thread reads "Running"
forever. `paused` is the honest state, and the user asked for it by name.
