-- Backfill the 'channel' field in event payloads where it is missing.
-- The channel field was added to EventMeta after some events were already stored.
-- Derive the correct channel from event_type for events that should always have one.

-- Claude Code events: SessionStarted, SessionResumed, ClaudeCodeIdled,
-- ClaudeCodeUserMessageSent, and all ClaudeCode* prefixed events
UPDATE events
SET payload = jsonb_set(payload, '{channel}', '"claude_code"')
WHERE payload->>'channel' IS NULL
  AND (
    event_type IN ('SessionStarted', 'SessionResumed', 'ClaudeCodeIdled', 'ClaudeCodeUserMessageSent')
    OR event_type LIKE 'ClaudeCode%'
  );

-- Scheduled task events
UPDATE events
SET payload = jsonb_set(payload, '{channel}', '"scheduled_task"')
WHERE payload->>'channel' IS NULL
  AND event_type IN ('ScheduledTaskStarted', 'ScheduledTaskCompleted');

-- Regular chat messages (MessageReceived without a channel is a chat message)
UPDATE events
SET payload = jsonb_set(payload, '{channel}', '"chat"')
WHERE payload->>'channel' IS NULL
  AND event_type = 'MessageReceived';
