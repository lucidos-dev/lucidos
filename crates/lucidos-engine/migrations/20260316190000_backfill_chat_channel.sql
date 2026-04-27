-- Backfill channel on origin events that have no channel set.
-- New events always have channel set explicitly; this covers legacy data.

UPDATE events
SET payload = jsonb_set(payload, '{channel}', '"chat"')
WHERE event_type = 'MessageReceived'
  AND (payload->>'channel') IS NULL;

UPDATE events
SET payload = jsonb_set(payload, '{channel}', '"scheduled_task"')
WHERE event_type = 'ScheduledTaskStarted'
  AND (payload->>'channel') IS NULL;
