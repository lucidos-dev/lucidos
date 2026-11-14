-- Projection table for thread metadata.
-- Maintained incrementally by EventBus on each relevant event,
-- replacing expensive CTE-based reconstruction from the events table.

CREATE TABLE IF NOT EXISTS thread_summaries (
    thread_id UUID PRIMARY KEY,
    title TEXT,
    first_message TEXT,
    source TEXT NOT NULL DEFAULT 'chat',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_activity TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    message_count INTEGER NOT NULL DEFAULT 0,
    is_pinned BOOLEAN NOT NULL DEFAULT FALSE,
    has_response BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX idx_thread_summaries_last_activity ON thread_summaries (last_activity DESC);
CREATE INDEX idx_thread_summaries_source_activity ON thread_summaries (source, last_activity DESC);
CREATE INDEX idx_thread_summaries_pinned ON thread_summaries (is_pinned) WHERE is_pinned = TRUE;

-- Backfill from existing events using the same logic as the old CTE queries.
INSERT INTO thread_summaries (thread_id, title, first_message, source, created_at, last_activity, message_count, is_pinned, has_response)
SELECT
    ts.thread_id,
    t.title,
    ts.first_message,
    ts.source,
    ts.created_at,
    COALESCE(la.last_activity, ts.created_at),
    COALESCE(mc.msg_count, 0),
    COALESCE(p.is_pinned, FALSE),
    COALESCE(r.has_response, FALSE)
FROM (
    SELECT DISTINCT ON (thread_id)
        thread_id,
        COALESCE(payload->>'content', payload->>'task_name') AS first_message,
        CASE
            WHEN event_type IN ('SessionStarted', 'SessionResumed') THEN COALESCE(payload->>'channel', 'claude_code')
            WHEN event_type = 'ScheduledTaskStarted' THEN COALESCE(payload->>'channel', 'scheduled_task')
            ELSE COALESCE(payload->>'channel', 'chat')
        END AS source,
        created AS created_at
    FROM events
    WHERE event_type IN ('MessageReceived', 'ScheduledTaskStarted', 'SessionStarted', 'SessionResumed')
      AND thread_id IS NOT NULL
    ORDER BY thread_id, created ASC
) ts
LEFT JOIN (
    SELECT thread_id, MAX(created) AS last_activity
    FROM events
    WHERE event_type IN ('MessageReceived', 'ResponseGenerated', 'ScheduledTaskStarted', 'SessionStarted', 'SessionResumed', 'ClaudeCodeIdled')
      AND thread_id IS NOT NULL
    GROUP BY thread_id
) la ON la.thread_id = ts.thread_id
LEFT JOIN (
    SELECT DISTINCT ON (thread_id)
        thread_id,
        payload->>'title' AS title
    FROM events
    WHERE event_type IN ('ThreadTitleRenamed', 'ThreadTitleGenerated')
      AND thread_id IS NOT NULL
    ORDER BY thread_id, created DESC
) t ON t.thread_id = ts.thread_id
LEFT JOIN (
    SELECT thread_id, COUNT(*) AS msg_count
    FROM events
    WHERE event_type IN ('MessageReceived', 'ScheduledTaskStarted', 'ClaudeCodeUserMessageSent')
      AND thread_id IS NOT NULL
    GROUP BY thread_id
) mc ON mc.thread_id = ts.thread_id
LEFT JOIN (
    SELECT p.thread_id, TRUE AS is_pinned
    FROM (
        SELECT thread_id, MAX(created) AS pinned_at
        FROM events WHERE event_type = 'ThreadPinned' AND thread_id IS NOT NULL
        GROUP BY thread_id
    ) p
    LEFT JOIN (
        SELECT thread_id, MAX(created) AS unpinned_at
        FROM events WHERE event_type = 'ThreadUnpinned' AND thread_id IS NOT NULL
        GROUP BY thread_id
    ) u ON u.thread_id = p.thread_id
    WHERE u.thread_id IS NULL OR p.pinned_at > u.unpinned_at
) p ON p.thread_id = ts.thread_id
LEFT JOIN (
    SELECT DISTINCT thread_id, TRUE AS has_response
    FROM events
    WHERE event_type = 'ResponseGenerated' AND thread_id IS NOT NULL
) r ON r.thread_id = ts.thread_id
ON CONFLICT (thread_id) DO NOTHING;
