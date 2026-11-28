-- Remove thread_id from event payloads now that the thread_id column is the source of truth.
-- This shrinks payload JSONB and eliminates redundant data.
UPDATE events SET payload = payload - 'thread_id'
WHERE payload ? 'thread_id' AND thread_id IS NOT NULL;

-- Drop the legacy functional index on payload->>'thread_id'.
-- All queries now use the thread_id column with idx_events_thread_seq.
DROP INDEX IF EXISTS idx_events_thread_id;
