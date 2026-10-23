ALTER TABLE thread_summaries ADD COLUMN total_children_count INTEGER NOT NULL DEFAULT 0;

-- Backfill: total = number of child threads that exist for each parent
UPDATE thread_summaries p
SET total_children_count = COALESCE(
    (SELECT COUNT(*) FROM thread_summaries c WHERE c.parent_thread_id = p.thread_id),
    0
)
WHERE EXISTS (SELECT 1 FROM thread_summaries c WHERE c.parent_thread_id = p.thread_id);
