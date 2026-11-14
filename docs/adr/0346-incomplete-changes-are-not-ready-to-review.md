# 0346: An incomplete change waits under Not finished, not Review, and still blocks Archive

- **Status**: Superseded by [ADR 0400](0400-coding-agent-change-state.md). An
  unfinished turn no longer proposes, so no pending change is incomplete.
- **Date**: 2026-10-02

## Context

ADR 0328 made a user Stop propose the branch's net work as an `incomplete`
change, so stopped work keeps an Apply. The row is an ordinary `pending` one
with a flag. Every surface that asks "is there a change to review?" reads
`coding_agent_proposed`, which counts every pending row. So a stopped thread
drew the "changes" dot, counted in the Review badge and led its banner with
Apply, exactly like finished work.

The case that exposed it: an agent committed a plan and asked for approval.
The user approved it and pressed Stop ten seconds later. The plan commit then
read as a change ready to review.

Plan: `docs/plans/2026-10-02-incomplete-changes-are-not-ready-to-review.md`.

## Decision

1. **An incomplete change is not ready to review.** It draws no "changes"
   dot, adds nothing to the Review badge or tier, and sits under **Not
   finished** in the Changes panel. Its thread banner leads with Continue,
   with Apply, Set aside and Discard in the caret menu.
2. **It still blocks Archive and Delete**, until the user applies, discards
   or sets it aside.
3. **The mark is a projection column, `thread_summaries.coding_agent_incomplete`.**
   The change row stays `pending`, and `coding_agent_proposed` keeps meaning
   "holds a pending change".

## Rationale

**Decision 2 costs nothing this way.** The archive, delete and family gates,
`is_blocking` and its SQL mirror all read `coding_agent_proposed`. Leaving its
meaning alone keeps every one of them right with no edit. Only the readers
that ask "ready to review?" learn the new column.

**Applying stays one path.** Apply, Apply Now, the conflict resolution and the
apply claim all guard on `pending` and read the data `ChangeState::Pending`
carries. An incomplete change still needs all of it, because the user can
apply it after a confirm.

**The column follows the rows.** `sync_thread_proposal` derives both columns
from the thread's pending rows in the event's transaction. A clean
re-proposal clears the flag on the row, and so the column, with no extra
event.

## Consequences

- Crash recovery's incomplete changes get the same treatment. Their work is
  just as unfinished.
- The word stays `incomplete` everywhere. `ChangeProposed.incomplete` is in
  immutable history, so a rename would leave two roots for one concept.
- The thread still surfaces in the inbox on the proposal. The user has
  something to deal with there, even though it is not ready.

## Alternatives considered

- **A new `unfinished` change status beside `set_aside`.** It would need its
  own copy of `Pending`'s data, or a "bring back" step before Apply. Every
  apply path would also learn a second status. Rejected: the flag already
  means this, and the status bought only a different place to store it.
- **Narrow `coding_agent_proposed` to complete changes only.** Every gate that
  blocks Archive and Delete would then need a second input, plus the SQL
  mirrors and the backfill. Rejected: it moves the part the user wants to keep.
- **Set the change aside on Stop.** It unblocks Archive, which the user wants
  kept. "Set aside" also reads as the user's own choice, not "the agent did
  not finish".
- **Derive it in the frontend from the changes list.** No migration, but on a
  cold load a stopped thread reads as ready until the list arrives.
