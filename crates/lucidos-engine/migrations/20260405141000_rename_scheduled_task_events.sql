-- Rename ScheduledTask* event types → ScheduledTrigger* (taxonomy overhaul Phase 1)

-- Event types in events table
UPDATE events SET event_type = 'ScheduledTriggerStarted' WHERE event_type = 'ScheduledTaskStarted';
UPDATE events SET event_type = 'ScheduledTriggerCompleted' WHERE event_type = 'ScheduledTaskCompleted';
UPDATE events SET event_type = 'ScheduledTriggerCreated' WHERE event_type = 'ScheduledTaskCreated';
UPDATE events SET event_type = 'ScheduledTriggerUpdated' WHERE event_type = 'ScheduledTaskUpdated';
UPDATE events SET event_type = 'ScheduledTriggerDeleted' WHERE event_type = 'ScheduledTaskDeleted';

-- Channel in event payloads (JSONB): "scheduled_task" → "scheduled_trigger"
UPDATE events
SET payload = jsonb_set(payload, '{channel}', '"scheduled_trigger"')
WHERE payload->>'channel' = 'scheduled_task';

-- Source in thread_summaries projection
UPDATE thread_summaries SET source = 'scheduled_trigger' WHERE source = 'scheduled_task';
