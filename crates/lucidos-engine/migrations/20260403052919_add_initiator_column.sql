-- Add initiator column to track WHO started the thread (user vs system).
-- Separate from source (channel) which tracks WHAT handles it.
ALTER TABLE thread_summaries ADD COLUMN IF NOT EXISTS initiator TEXT NOT NULL DEFAULT 'user';

-- Backfill: scheduled_task threads are system-initiated
UPDATE thread_summaries SET initiator = 'system' WHERE source = 'scheduled_task';

-- Backfill: child threads of system-initiated threads inherit 'system'
UPDATE thread_summaries AS child
SET initiator = 'system'
FROM thread_summaries AS parent
WHERE child.parent_thread_id = parent.thread_id
  AND parent.initiator = 'system'
  AND child.initiator = 'user';
