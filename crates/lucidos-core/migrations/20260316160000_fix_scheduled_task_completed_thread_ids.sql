-- Fix historical data: ScheduledTaskCompleted events were incorrectly assigned
-- thread_id values from event payloads by migration 20260314080000. These events
-- are standalone bookkeeping events and should NOT belong to any thread.
-- The code was fixed (Event::scheduled_task_completed sets thread_id=None),
-- but 11k+ historical events still have wrong thread_id/aggregate_id.

UPDATE events
SET thread_id = NULL, aggregate_id = NULL
WHERE event_type = 'ScheduledTaskCompleted'
  AND thread_id IS NOT NULL;
