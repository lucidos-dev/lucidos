-- Add `wake_mode` to `thread_summaries`: what a child's clean finish does to
-- its parent, declared by the spawn (ADR 0451).
--
-- `no-wake` files every clean report. `wake` wakes the parent for every
-- report. `wake-when-all-done` wakes it once no other child of its batch is
-- still working. A failure wakes the parent under every mode.
--
-- Written once, by the spawning `MessageReceived`, in the statement that
-- writes `parent_thread_id`. NULL means the spawn declared nothing, so the
-- child's turns fall back to the wake decision.
--
-- No backfill. Every existing child keeps the wake decision it had.

ALTER TABLE thread_summaries
  ADD COLUMN wake_mode TEXT
    CONSTRAINT thread_summaries_wake_mode_valid
      CHECK (wake_mode IN ('no-wake', 'wake', 'wake-when-all-done'));
