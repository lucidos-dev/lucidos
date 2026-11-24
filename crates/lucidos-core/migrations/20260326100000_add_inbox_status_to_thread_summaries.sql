-- Add inbox_status to thread_summaries for tracking which threads need attention.
-- Values: 'none' (default/read), 'waiting' (blocked on child threads), 'unread' (has new results).
ALTER TABLE thread_summaries ADD COLUMN IF NOT EXISTS inbox_status TEXT NOT NULL DEFAULT 'none';

-- Index for querying inbox threads (waiting + unread) efficiently.
CREATE INDEX IF NOT EXISTS idx_thread_summaries_inbox
    ON thread_summaries (inbox_status, last_activity DESC)
    WHERE inbox_status != 'none';

-- Backfill: mark threads with active children as 'waiting'.
-- A parent thread is 'waiting' if it has children that haven't completed yet
-- (i.e., children whose last event is NOT a completion event).
-- This is conservative — we only backfill clear cases.
UPDATE thread_summaries parent
SET inbox_status = 'waiting'
WHERE EXISTS (
    SELECT 1 FROM thread_summaries child
    WHERE child.parent_thread_id = parent.thread_id
)
AND inbox_status = 'none'
AND parent.parent_thread_id IS NULL;
