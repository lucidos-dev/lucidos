-- Add trigger_id to thread_summaries so the drawer filter can show/hide
-- threads from individual triggers without lazy-loading every thread's events.
-- Mirrors how parent_thread_id is exposed: a single column projection-side,
-- joined in once at write time.
ALTER TABLE thread_summaries
    ADD COLUMN IF NOT EXISTS trigger_id UUID NULL;

-- Backfill: pull trigger_id from each thread's first TriggerStarted event.
-- The UUID regex guards against legacy non-UUID identifiers (e.g. test
-- fixtures or pre-UUID dev data) that would otherwise crash the cast.
UPDATE thread_summaries ts
SET trigger_id = (e.payload->>'trigger_id')::uuid
FROM events e
WHERE e.thread_id = ts.thread_id
  AND e.event_type = 'TriggerStarted'
  AND ts.trigger_id IS NULL
  AND e.payload->>'trigger_id' ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$';

CREATE INDEX IF NOT EXISTS idx_thread_summaries_trigger_id
    ON thread_summaries (trigger_id)
    WHERE trigger_id IS NOT NULL;
