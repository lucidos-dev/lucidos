ALTER TABLE thread_summaries
  ADD COLUMN trigger_id text,
  ADD COLUMN trigger_name text;

-- Backfill from the *first* TriggerStarted per thread, matching the runtime
-- COALESCE-on-conflict semantics (first write wins).
UPDATE thread_summaries ts
SET trigger_id = first_evt.trigger_id,
    trigger_name = first_evt.trigger_name
FROM (
  SELECT DISTINCT ON (e.aggregate_id)
    e.aggregate_id,
    e.payload->>'trigger_id'   AS trigger_id,
    e.payload->>'trigger_name' AS trigger_name
  FROM events e
  WHERE e.event_type = 'TriggerStarted'
  ORDER BY e.aggregate_id, e.created ASC
) AS first_evt
WHERE first_evt.aggregate_id = ts.thread_id::text
  AND ts.source = 'trigger';

CREATE INDEX idx_thread_summaries_trigger_id
  ON thread_summaries(trigger_id) WHERE trigger_id IS NOT NULL;
