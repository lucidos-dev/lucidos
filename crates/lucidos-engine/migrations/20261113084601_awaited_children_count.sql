-- Replace `waiting_children_count` with the awaited-child projection (ADR 0672).
--
-- `awaited_by_parent`: this row's parent waits on it. Its finish wakes the
-- parent (wake mode `wake`, `wake-when-all-done` or NULL), and it is
-- unfinished: active, holding a live event wait, or awaiting a child itself.
--
-- `awaited_children_count`: how many direct children have the flag set. The
-- last clause above reads it, so the roll-up climbs any depth.
--
-- No backfill here. The boot rebuild (`rebuild_children_counts`) computes both
-- bottom-up on every start, with the predicate the live projection uses.

ALTER TABLE thread_summaries
  ADD COLUMN awaited_by_parent BOOLEAN NOT NULL DEFAULT FALSE,
  ADD COLUMN awaited_children_count INT NOT NULL DEFAULT 0,
  DROP COLUMN waiting_children_count;
