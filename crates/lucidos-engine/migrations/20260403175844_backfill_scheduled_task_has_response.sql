-- Backfill has_response for scheduled task threads that completed but were
-- never marked visible (ScheduledTaskCompleted didn't set has_response=TRUE).
UPDATE thread_summaries
SET has_response = TRUE
WHERE source = 'scheduled_task'
  AND has_response = FALSE
  AND thread_id IN (
      SELECT DISTINCT thread_id FROM events
      WHERE event_type = 'ScheduledTaskCompleted' AND thread_id IS NOT NULL
  );
