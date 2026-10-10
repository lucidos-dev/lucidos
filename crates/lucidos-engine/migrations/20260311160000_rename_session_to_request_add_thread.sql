-- Rename session_id -> request_id in all event payloads
UPDATE events
SET payload = payload - 'session_id' || jsonb_build_object('request_id', payload->>'session_id')
WHERE payload ? 'session_id';

-- Backfill thread_id = request_id for all existing events (each historical message is its own thread)
UPDATE events
SET payload = payload || jsonb_build_object('thread_id', payload->>'request_id')
WHERE payload ? 'request_id' AND NOT payload ? 'thread_id';

-- Drop old index
DROP INDEX IF EXISTS idx_events_session_id;

-- Create renamed index
CREATE INDEX IF NOT EXISTS idx_events_request_id
ON events ((payload->>'request_id'))
WHERE payload->>'request_id' IS NOT NULL;

-- Create thread_id index
CREATE INDEX IF NOT EXISTS idx_events_thread_id
ON events ((payload->>'thread_id'))
WHERE payload->>'thread_id' IS NOT NULL;

-- Rename session_id -> request_id in changes table
ALTER TABLE changes RENAME COLUMN session_id TO request_id;
