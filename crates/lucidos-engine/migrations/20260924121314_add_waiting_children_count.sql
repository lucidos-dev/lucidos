-- Add `waiting_children_count` to `thread_summaries`.
--
-- How many direct children are not in flight and hold a live *event wait*.
-- Such a child has not finished (ADR 0254), so its parent is waiting on it.
-- Kept apart from `active_children_count`, which means "running": that count
-- also feeds the section, the fan-in and worktree cleanup, and ADR 0254
-- rejected widening it. This one feeds only the frontend's Waiting dot and
-- waiting indicator.
--
-- The backfill uses the same ground-truth COUNT as the projection's
-- `reconcile_parent_waiting_children_count` and the boot rebuild.

ALTER TABLE thread_summaries
  ADD COLUMN waiting_children_count INT NOT NULL DEFAULT 0;

UPDATE thread_summaries p
SET waiting_children_count = w.cnt
FROM (
    SELECT parent_thread_id, COUNT(*)::int AS cnt
    FROM thread_summaries
    WHERE parent_thread_id IS NOT NULL
      AND live_event_wait_count > 0
      AND status NOT IN ('running', 'waiting_for_user_answer')
    GROUP BY parent_thread_id
) w
WHERE p.thread_id = w.parent_thread_id;
