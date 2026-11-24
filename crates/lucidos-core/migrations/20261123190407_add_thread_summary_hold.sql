-- The hold standing on a thread, as the drawer and the composer strip read it
-- (ADR 0805). NULL when the thread is not on hold. The events stay the
-- authority: `core::store::thread_hold` folds the same four event types, and
-- the thread projection keeps this column in step with them.
ALTER TABLE thread_summaries ADD COLUMN IF NOT EXISTS hold JSONB;

-- Backfill: the newest of the four hold event types on each thread wins. A
-- retired pause folds as a hold rooted on its own thread.
WITH newest AS (
    SELECT DISTINCT ON (thread_id)
           thread_id, event_type, created, payload
    FROM events
    WHERE event_type IN ('ThreadHoldPlaced', 'ThreadHoldLifted',
                         'AutomationPaused', 'AutomationUnpaused')
      AND thread_id IS NOT NULL
    ORDER BY thread_id, sequence DESC
)
UPDATE thread_summaries ts
SET hold = jsonb_strip_nulls(jsonb_build_object(
        'reason', COALESCE(n.payload->>'reason', ''),
        'root_thread_id', CASE
            WHEN n.event_type = 'ThreadHoldPlaced'
                THEN COALESCE(n.payload->>'root_thread_id', n.thread_id::text)
            ELSE n.thread_id::text
        END,
        'placed_at', to_jsonb(n.created),
        'placed_by', n.payload->'actor'))
FROM newest n
WHERE ts.thread_id = n.thread_id
  AND n.event_type IN ('ThreadHoldPlaced', 'AutomationPaused');
