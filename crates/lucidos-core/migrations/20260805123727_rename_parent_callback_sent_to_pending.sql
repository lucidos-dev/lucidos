-- Rename `parent_callback_sent` to `parent_callback_pending` and invert it.
-- The old name states a permanent historical fact ("a card was sent for this
-- child") while the semantics are per-run: the marker is cleared every time
-- the child is revived, so what it actually tracks is whether the parent has
-- been told about the child's CURRENT turn. The flipped name says that, and it
-- makes the retry write in `notify_parent_if_child` honest: when the parent row
-- is missing the card IS persisted, so "no callback was sent" was false, while
-- "the parent callback is still pending" (the wake never reached the channel)
-- is exactly true. See ADR 0011 and D12 of
-- docs/plans/2026-08-05-parent-follows-up-on-its-own-child-thread.md.
--
-- NOT NULL and DEFAULT FALSE carry across the rename and are both still wanted:
-- a thread with no parent has no parent callback pending, which under the new
-- name is a true statement rather than a merely harmless one. TRUE is written
-- explicitly, only for a thread that has a parent.
ALTER TABLE thread_summaries
  RENAME COLUMN parent_callback_sent TO parent_callback_pending;

-- Backfill. For a child row the mapping is exact: old FALSE (no card yet for
-- this run) becomes TRUE (pending), old TRUE (card delivered) becomes FALSE. A
-- child mid-run when this lands still gets its card; a child that already
-- reported still has its extra idles swallowed. The `parent_thread_id IS NOT
-- NULL` conjunct keeps every parentless row at FALSE regardless of what it
-- held; today they all hold FALSE already (both TRUE-writers are reached only
-- for a thread with a parent), so it is defence in depth, not a repair.
UPDATE thread_summaries
   SET parent_callback_pending = (parent_thread_id IS NOT NULL)
                                 AND NOT parent_callback_pending;
