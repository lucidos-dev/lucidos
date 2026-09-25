# 0279: A parent reads its sub-threads' pending changes from one loader and one sub-thread definition

- **Status**: Accepted
- **Date**: 2026-09-24

## Context

On 2026-09-24 an orchestrating chat thread misread its sub-threads' work twice.

- **"Pending changes: none" over a subtree full of them.** An orchestrator's
  children each held a pending change. Every `ChildThreadCompleted` it sent its
  parent read `Pending changes: none`, because the fan-in read only the
  reporting thread's own branch.
- **A running session read as settled.** The `changes` tool listed a change
  with `thread_unsettled` and `thread_settling` false while its thread was
  running a new turn. Later that evening it listed a change as settled while
  its thread resolved a merge conflict, and `apply` on the same change was
  refused. The master agent told the user a session was done.

The second cause was not stale state. The flags are not columns. They are
filled at read time by `enrich_pending_state`, and a direct projection load
leaves them `false`. The HTTP list and the SSE frame filled them. The tool did
not. The SSE frame had drifted the same way once before, so this was the second
reader to skip the step.

## Decision

1. **One loader serves every pending list that leaves the engine.**
   `core::changes::list_pending_for_readers` lists, titles and fills the
   thread-state flags. The HTTP list, the `ChangesUpdated` frame and the
   `changes` tool all call it. `enrich_pending_state` is private to it, and a
   source guard fails if a reader lists pending changes without it.
2. **A completion card reports the subtree, apart from the child's own.**
   `ChildThreadCompleted` gains `sub_thread_pending_changes`. Each entry names
   the change, its sub-thread, the sub-thread's title and `thread_unsettled`.
   `pending_change_ids` stays the child's own branch.
3. **One SQL definition of "sub-thread" for changes.** `SUB_THREADS_CTE` backs
   the card, a read-time `pending_sub_thread_change_count` on
   `list_thread_summaries` rows, and a `sub_threads_of` filter on the changes
   list (HTTP, tool and CLI).
4. **No new "still working" field.** With the flags filled, no thread reads
   `running` in `threads list` while its change reads settled. `paused` reads
   unsettled false and settling true, and `threads list` shows it as `paused`.
   So `thread_unsettled` already means "still working", and the docs say so.

## Rationale

A flag that defaults to `false` reads as "settled" wherever someone forgets to
fill it. Fixing the one reader would leave the next one free to forget. Making
the fill step private moves the question from memory to the compiler, and the
source guard covers a reader that never asks at all.

The card is a snapshot, and it cannot be anything else: events are immutable.
It carries the settled state as of the card and says so, which is honest. The
live answer stays one `changes` list call away, now with a subtree filter.

Keeping the two lists apart matters because they answer different questions.
The child's own change is what the parent may apply for the child. A sub-thread
change belongs to a thread the parent may not have spawned directly, and the
parent needs its owner to act on it.

## Consequences

- The completion block grows a section only when a sub-thread holds a pending
  change. Every other card reads as before.
- `pending_sub_thread_change_count` is present on `threads list` rows and
  absent elsewhere. An absent field never claims zero.
- A read error on the sub-thread query logs and sends the card without the
  section, the same as the own-branch lookup already does.
- The SSE frame now skips a refresh when the flags cannot be filled, rather
  than sending rows whose Apply looks live. The next emit retries.

## Alternatives considered

- **A maintained column for the sub-thread count**, like
  `blocking_descendant_count`. Correct on every read path, but it needs
  propagation on every change event, detach and archive. A read-time count on
  the one list an agent reads its children from costs one query and cannot
  drift.
- **Folding sub-thread changes into `pending_change_ids`.** It fixes the
  "none", but the parent can no longer tell whose change is whose, and the id
  alone names no owner.
- **Storing the flags as columns.** They depend on the thread's live status and
  waits, which change without any change event. A column would need a write
  on every thread status change, for a value one indexed query already answers.
- **A separate `thread_status` field on each change.** It would make a reader
  cross-check two meanings of one fact. The flags already say it once the
  loader fills them.
