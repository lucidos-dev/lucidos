-- Add last_revived_at to track when a thread last entered the 'running' state.
-- Used to sort IN PROGRESS threads by when they were started/revived.
ALTER TABLE thread_summaries ADD COLUMN IF NOT EXISTS last_revived_at TIMESTAMPTZ;

-- Backfill: threads currently running get last_activity as their revived time.
UPDATE thread_summaries SET last_revived_at = last_activity WHERE status = 'running';
