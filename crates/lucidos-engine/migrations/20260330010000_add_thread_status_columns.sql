-- Add status and CC state columns to thread_summaries.
-- Status is now computed by the backend (EventBus) instead of the frontend.

ALTER TABLE thread_summaries ADD COLUMN status TEXT NOT NULL DEFAULT 'idle';
ALTER TABLE thread_summaries ADD COLUMN cc_has_changes BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE thread_summaries ADD COLUMN cc_requires_restart BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE thread_summaries ADD COLUMN cc_is_external_repo BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE thread_summaries ADD COLUMN cc_applying BOOLEAN NOT NULL DEFAULT FALSE;

-- Backfill: threads whose last relevant event is ClaudeCodeIdled with unresolved
-- changes should be 'waiting'. Don't set 'running' — that's ephemeral state set
-- by the engine when a thread task is active.

-- Find CC threads where the most recent terminal/action_required event is ClaudeCodeIdled
-- and there are pending changes (ChangeProposed without matching Applied/Discarded).
WITH cc_waiting AS (
    SELECT DISTINCT ON (e.aggregate_id) e.aggregate_id AS thread_id,
           e.payload->>'has_changes' AS has_changes,
           e.payload->>'requires_restart' AS requires_restart,
           e.payload->>'is_external_repo' AS is_external_repo
    FROM events e
    WHERE e.event_type IN (
        'ClaudeCodeIdled', 'SessionEnded', 'ChangeApplied', 'ChangeDiscarded',
        'ResponseGenerated', 'ResponseCanceled', 'ResponseAborted'
    )
    AND e.aggregate_id IS NOT NULL
    AND e.aggregate = 'thread'
    ORDER BY e.aggregate_id, e.sequence DESC
),
waiting_threads AS (
    SELECT thread_id,
           has_changes,
           requires_restart,
           is_external_repo
    FROM cc_waiting
    WHERE has_changes = 'true'
)
UPDATE thread_summaries ts
SET status = 'waiting',
    cc_has_changes = TRUE,
    cc_requires_restart = COALESCE(wt.requires_restart = 'true', FALSE),
    cc_is_external_repo = COALESCE(wt.is_external_repo = 'true', FALSE)
FROM waiting_threads wt
WHERE ts.thread_id::text = wt.thread_id;

-- Also set 'waiting' for threads whose last event is ClaudeCodeIdled (even without changes,
-- since the CC session is idle and needs user attention), but only if the thread is in 'unread' section.
WITH cc_idle AS (
    SELECT DISTINCT ON (e.aggregate_id) e.aggregate_id AS thread_id,
           e.event_type
    FROM events e
    WHERE e.event_type IN (
        'ClaudeCodeIdled', 'SessionEnded', 'ChangeApplied', 'ChangeDiscarded',
        'ResponseGenerated', 'ResponseCanceled', 'ResponseAborted'
    )
    AND e.aggregate_id IS NOT NULL
    AND e.aggregate = 'thread'
    ORDER BY e.aggregate_id, e.sequence DESC
)
UPDATE thread_summaries ts
SET status = 'waiting'
FROM cc_idle ci
WHERE ts.thread_id::text = ci.thread_id
  AND ci.event_type = 'ClaudeCodeIdled'
  AND ts.section = 'unread'
  AND ts.status = 'idle';
