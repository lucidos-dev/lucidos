-- Rename ScheduledTrigger* events → Trigger* (vocabulary cleanup: triggers
-- are scheduled OR event-driven OR hybrid; the channel is "trigger", with
-- the actual invocation recorded on the event payload).

-- Event types in events table
UPDATE events SET event_type = 'TriggerStarted' WHERE event_type = 'ScheduledTriggerStarted';
UPDATE events SET event_type = 'TriggerCompleted' WHERE event_type = 'ScheduledTriggerCompleted';

-- Channel in event payloads (JSONB): "scheduled_trigger" → "trigger"
UPDATE events
SET payload = jsonb_set(payload, '{channel}', '"trigger"')
WHERE payload->>'channel' = 'scheduled_trigger';

-- Source in thread_summaries projection
UPDATE thread_summaries SET source = 'trigger' WHERE source = 'scheduled_trigger';

-- Backfill TriggerStarted.invocation for historical rows. We can't tell from
-- the row alone whether a past run was fired by cron or by an event, so we
-- default to "Schedule" (which matches the prior naming and the predominant
-- case). The popover will render this as "Scheduled".
UPDATE events
SET payload = jsonb_set(
    payload,
    '{invocation}',
    '{"kind":"Schedule"}'::jsonb
)
WHERE event_type = 'TriggerStarted'
  AND NOT (payload ? 'invocation');
