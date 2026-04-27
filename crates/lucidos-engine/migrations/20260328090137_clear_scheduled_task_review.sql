-- Move existing scheduled task threads out of REVIEW (unread) section.
-- The engine now prevents new scheduled tasks from entering REVIEW,
-- but threads that arrived before the fix are stuck there.
UPDATE thread_summaries
SET section = 'default'
WHERE section = 'unread'
  AND source = 'scheduled_task';
