-- Backfill channel="scheduled_task" on all events in scheduled task threads.
-- The origin ScheduledTaskStarted events were already backfilled by
-- 20260316190000_backfill_chat_channel.sql, but response events
-- (ResponseGenerated, ToolCalled, ToolResult, etc.) in those same threads
-- were missing the channel field. Without it, the memory indexer and
-- rebuild path can't identify them as scheduled task events to skip.

UPDATE events
SET payload = jsonb_set(payload, '{channel}', '"scheduled_task"')
WHERE aggregate_id IN (
    SELECT DISTINCT aggregate_id FROM events
    WHERE event_type = 'ScheduledTaskStarted'
      AND aggregate_id IS NOT NULL
)
AND (payload->>'channel') IS NULL;
