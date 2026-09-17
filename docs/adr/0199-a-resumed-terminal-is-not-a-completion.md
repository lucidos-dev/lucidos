# 0199: The fan-in withholds a completion card for a terminal the engine is resuming

- **Status**: Accepted
- **Date**: 2026-09-16

## Context

A coding-agent turn that dies on a transient upstream `API Error` emits a real
`ResponseFailed`. The child-to-parent fan-in runs inside that emit's PostCommit
phase, so the parent gets a `ChildThreadCompleted` with `status: failure` right
there. The engine then resumes the same session at the idle exit, seconds later.

A parent believed such a card. It checked the child's branch and saw zero
commits, the work being uncommitted in the worktree. It then spawned a duplicate
session onto the same scroll path. Two agents edited the same files until a human
noticed.

The recovery path was correct throughout. The notification path was not.

## Decision

A terminal the engine has already decided to auto-resume emits no completion
card, no parent wake, and no parent callback.

The decision is taken ONCE, in `hold_completion_if_api_error_resume`, at the
moment the terminal is classified and before it is emitted. It registers an
*auto-resume hold*, and `notify_parent_if_child` stands down while the hold is
set. `maybe_auto_resume_after_api_error` actuates that decision rather than
re-deciding.

**The hold spans two emits and no more.** The run loop drops it once the
terminal and its `CodingAgentIdled` are out. What the release hands back is the
decision, and it resets with `last_terminal_kind`.

## Rationale

The card and the resume were two decisions taken at two moments, and nothing
connected them. Making them one decision is the only shape that cannot drift.

The resume emit cannot move earlier to meet the card. Its position is already
load-bearing twice over. A `ContinuationRequested` is re-dispatched at startup
only while no coding-agent lifecycle event follows it. And emitting it while the
subprocess is alive races the spawn dispatcher into a session about to be
cancelled. So the decision moves earlier and the emit stays.

**Suppression keys on the hold, never on the error class.** Past
`MAX_API_ERROR_AUTO_RESUMES` the thread parks for good and no hold is taken, so
the card fires. A silent dead child is a worse bug than the one being fixed.

**The hold touches nothing durable.** It never clears
`thread_summaries.parent_callback_pending`, the record that the child still owes
its parent a card. That is what lets the real terminal report, and it is why an
engine death mid-hold is safe: the hold vanishes, the marker does not, and
recovery's resumed turn reports normally.

## Consequences

- The parent is told once, at the real terminal. Before this, each transient
  failure emitted a card and the following `ContinuationRequested` re-marked the
  callback pending, so three auto-resumes produced four cards.
- A decision nobody actuated still reaches the parent. When the continuation does
  not persist, `held_completion_release` answers `Announce` and the engine
  replays the withheld `ResponseFailed` through the fan-in. An engine shutdown
  counts: a bare stop carries no device actor, so recovery parks the thread
  behind a manual Continue rather than resuming it.
- The register is in memory. It coordinates across two emits of one turn, in the
  same class as a cancellation token. Its loss is covered by the durable marker.
- The register is keyed by thread, with no session-identity token. Two live runs
  on one thread would read each other's hold. The window is one `select!` arm.
  `own_session_entry` already treats that overlap as real, so this is a bound to
  know rather than one the change closes.
- A hold cannot outlive the terminal it names. Keying by thread makes a stale one
  dangerous: a `KeepAlive` at idle carries the loop through another whole turn,
  and a Stop or a safety net there has to report normally. Releasing inside the
  arm that took it, rather than at the resume site, is what rules that out.

## Alternatives considered

**Re-derive the predicate in the fan-in.** The fan-in has the pool and the error
string, so it could ask `is_transient_api_failure` and count the budget itself.
Rejected: it would be a second copy of a decision the engine already takes, and
the two would disagree on the inputs the fan-in cannot see. A shutdown beginning
mid-turn and a conflict-resolution session both refuse the resume, and neither is
visible from the bus. Each divergence is a silently dead child.

**Add a field to `ResponseFailed` saying the turn is being retried.**
Event-sourced and honest, and rejected on cost. It changes a persisted wire
contract, so it drags the generated TypeScript, the knowhow doc and the card
renderer. It also has to be repeated on `CodingAgentIdled`, which follows the
failure and would otherwise announce the same turn as a success.

**Emit no terminal at all, as the watchdog path does.** The watchdog kills the
subprocess before any Result, so there is nothing to emit. Here the backend
reported the failure itself, and dropping it would erase the drop from the
transcript. The idle is load-bearing besides: the continuation's re-dispatch
ordering depends on it.

**Deliver the card and retract it when the resume lands.** There is no
retraction. Events are append-only, and the parent has already acted on what it
read.
