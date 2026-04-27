-- Add first-class thread_id column (nullable for existing events without threads)
ALTER TABLE events ADD COLUMN IF NOT EXISTS thread_id UUID;

-- Add monotonic sequence for ordering and dedup
ALTER TABLE events ADD COLUMN IF NOT EXISTS sequence BIGSERIAL;

-- Backfill thread_id from payload JSONB
UPDATE events SET thread_id = (payload->>'thread_id')::UUID
WHERE thread_id IS NULL AND payload->>'thread_id' IS NOT NULL;

-- Primary query index: all events for a thread, ordered
CREATE INDEX IF NOT EXISTS idx_events_thread_seq ON events (thread_id, sequence)
WHERE thread_id IS NOT NULL;
