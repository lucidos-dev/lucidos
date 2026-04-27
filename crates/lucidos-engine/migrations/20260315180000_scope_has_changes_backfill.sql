-- Fix overly aggressive backfill from 20260315140000.
-- Set has_changes=false for ClaudeCodeIdled events whose thread has no real
-- git branch (UUID session IDs from CcThreadSpawned, or no SessionStarted).

UPDATE events e
SET payload = jsonb_set(payload, '{has_changes}', 'false')
WHERE e.event_type = 'ClaudeCodeIdled'
  AND (e.payload->>'has_changes') = 'true'
  AND NOT EXISTS (
    SELECT 1 FROM events s
    WHERE s.thread_id = e.thread_id
      AND s.event_type = 'SessionStarted'
      AND s.payload->>'session_id' LIKE 'claude-code/%'
  );
