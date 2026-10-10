-- Backfill missing ResponseCancelled events for old CC exchanges.
--
-- Before the fix in 6cd4d3b8, the backend didn't emit ResponseCancelled when
-- a CC follow-up was stopped/cancelled. These exchanges have a user message
-- (MessageReceived or ClaudeCodeUserMessageSent) followed by ClaudeCodeIdled
-- without any terminal event (ResponseGenerated/ResponseCancelled/ResponseFailed/
-- SessionEnded) in between.
--
-- The frontend sees ClaudeCodeIdled and shows "Waiting" instead of "Canceled".
-- This migration inserts ResponseCancelled just before ClaudeCodeIdled to fix
-- the display for old data.

INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id)
SELECT
    gen_random_uuid(),
    'ResponseCancelled',
    '{}'::jsonb,
    idle.created - interval '1 millisecond',
    'thread',
    idle.aggregate_id,
    idle.aggregate_id::uuid
FROM events idle
WHERE idle.event_type = 'ClaudeCodeIdled'
  -- Must be in a thread with a CC session (has SessionStarted)
  AND EXISTS (
      SELECT 1 FROM events ss
      WHERE ss.aggregate_id = idle.aggregate_id
        AND ss.event_type = 'SessionStarted'
  )
  -- Find the most recent exchange boundary before this idle
  AND EXISTS (
      SELECT 1 FROM events boundary
      WHERE boundary.aggregate_id = idle.aggregate_id
        AND boundary.sequence < idle.sequence
        AND boundary.event_type IN ('MessageReceived', 'ClaudeCodeUserMessageSent')
        -- No terminal event between the boundary and this idle
        AND NOT EXISTS (
            SELECT 1 FROM events terminal
            WHERE terminal.aggregate_id = idle.aggregate_id
              AND terminal.sequence > boundary.sequence
              AND terminal.sequence < idle.sequence
              AND terminal.event_type IN (
                  'ResponseGenerated', 'ResponseCancelled', 'ResponseFailed', 'SessionEnded'
              )
        )
        -- The boundary must be the closest one before this idle
        -- (i.e., no other boundary between it and the idle)
        AND NOT EXISTS (
            SELECT 1 FROM events closer
            WHERE closer.aggregate_id = idle.aggregate_id
              AND closer.sequence > boundary.sequence
              AND closer.sequence < idle.sequence
              AND closer.event_type IN ('MessageReceived', 'ClaudeCodeUserMessageSent')
        )
  );
