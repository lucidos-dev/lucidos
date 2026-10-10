-- Track thread nesting depth for recursion guard.
-- Root threads have depth 0, children get parent_depth + 1.
ALTER TABLE thread_summaries ADD COLUMN IF NOT EXISTS depth INTEGER NOT NULL DEFAULT 0;

-- Backfill: threads with a parent get depth 1 (best effort — deeper chains
-- are rare since this is a new feature, and the guard will set depth correctly
-- going forward).
UPDATE thread_summaries SET depth = 1 WHERE parent_thread_id IS NOT NULL AND depth = 0;
