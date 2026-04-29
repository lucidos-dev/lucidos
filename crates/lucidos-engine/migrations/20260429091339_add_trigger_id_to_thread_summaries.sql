-- Add trigger_id to thread_summaries so the drawer filter can show/hide
-- threads from individual triggers without lazy-loading every thread's events.
-- Mirrors how parent_thread_id is exposed: a single column projection-side,
-- joined in once at write time.
ALTER TABLE thread_summaries
    ADD COLUMN IF NOT EXISTS trigger_id UUID NULL;

-- Backfill: pull trigger_id from each thread's earliest TriggerStarted event.
-- DISTINCT ON + ORDER BY created ASC matches event_bus.rs, which uses
-- COALESCE(thread_summaries.trigger_id, EXCLUDED.trigger_id) at write time
-- to lock in the first observed trigger. UUID regex guards against legacy
-- non-UUID identifiers that would crash the cast.
UPDATE thread_summaries ts
SET trigger_id = first_trigger.trigger_id
FROM (
    SELECT DISTINCT ON (e.thread_id)
        e.thread_id,
        (e.payload->>'trigger_id')::uuid AS trigger_id
    FROM events e
    WHERE e.event_type = 'TriggerStarted'
      AND e.payload->>'trigger_id' ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
    ORDER BY e.thread_id, e.created ASC
) AS first_trigger
WHERE ts.thread_id = first_trigger.thread_id
  AND ts.trigger_id IS NULL;

CREATE INDEX IF NOT EXISTS idx_thread_summaries_trigger_id
    ON thread_summaries (trigger_id)
    WHERE trigger_id IS NOT NULL;
