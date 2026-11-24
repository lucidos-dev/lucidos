-- Add `parent_wake_requested` to `thread_summaries`: the child's *wake
-- decision* for its current turn.
--
-- TRUE when the child said its parent must act on this turn's report
-- (`ParentWakeRequested`), FALSE when it said the parent need not
-- (`ParentWakeNotRequested`). NULL while the turn has not decided. Every turn
-- start resets it to NULL, in the statement that sets
-- `parent_callback_pending`, so a decision binds one turn only.
--
-- Only FALSE files a report. The fan-in wakes the parent on TRUE, on NULL, and
-- on any finish that is not clean.
--
-- No backfill. NULL is the waking side, which is what every existing child
-- did before this column.

ALTER TABLE thread_summaries
  ADD COLUMN parent_wake_requested BOOLEAN;
