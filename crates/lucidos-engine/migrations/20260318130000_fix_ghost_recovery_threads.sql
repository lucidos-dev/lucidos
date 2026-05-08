-- Fix ghost recovery threads: add SessionEnded events for recovery threads
-- that completed with ClaudeCodeIdled but were never properly ended.
-- These show as WAITING ghost threads with "..." title.
--
-- Also backfill branch names in SessionStarted events using the changes table,
-- so future recovery can map branch → original thread.

-- 1. End ghost recovery threads: insert SessionEnded for threads whose last
--    lifecycle event is ClaudeCodeIdled AND that have a SessionResumed event
--    (i.e., they are recovery threads, not user-initiated idle sessions).
INSERT INTO events (id, event_type, payload, created, thread_id)
SELECT
    gen_random_uuid(),
    'SessionEnded',
    '{"channel": "claude_code"}'::jsonb,
    NOW(),
    sub.thread_id
FROM (
    SELECT DISTINCT ON (thread_id) thread_id, event_type
    FROM events
    WHERE event_type IN ('ClaudeCodeIdled', 'SessionEnded')
      AND thread_id IS NOT NULL
    ORDER BY thread_id, sequence DESC
) sub
WHERE sub.event_type = 'ClaudeCodeIdled'
  AND sub.thread_id IN (
    SELECT DISTINCT thread_id FROM events
    WHERE event_type = 'SessionResumed' AND thread_id IS NOT NULL
  );

-- 2. Backfill branch in SessionStarted events from the changes table.
--    For each thread that has a change with branch_name, update the SessionStarted
--    event payload to include the branch.
UPDATE events e
SET payload = e.payload || jsonb_build_object('branch', c.branch_name)
FROM changes c
WHERE e.event_type = 'SessionStarted'
  AND e.thread_id = c.thread_id
  AND e.thread_id IS NOT NULL
  AND (e.payload->>'branch' IS NULL OR e.payload->>'branch' = '');
