-- Add `is_stopped_child` to `thread_summaries` (ADR 0252).
--
-- TRUE while a user Stop has ended a child thread's turn and nothing has
-- settled it yet. The parent has seen a `ChildThreadStopped` note, and its
-- `ChildThreadCompleted` is still owed. The `ResponseCanceled { user_stop }`
-- arm sets it. A start event on the child, or the settling
-- `ChildThreadCompleted`, clears it.
--
-- It feeds `is_attention_needing` and NOT `is_blocking`: a stopped child needs
-- the user, but archiving its parent must stay possible.
--
-- No backfill. Before this column, every user Stop on a child already sent the
-- parent a canceled card and cleared `parent_callback_pending`, so no existing
-- row is owed anything.

ALTER TABLE thread_summaries
  ADD COLUMN is_stopped_child BOOLEAN NOT NULL DEFAULT FALSE;
