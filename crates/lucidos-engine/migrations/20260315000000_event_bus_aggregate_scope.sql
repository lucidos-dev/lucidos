-- Add aggregate and aggregate_id columns for EventBus.
-- aggregate: which aggregate this event belongs to (thread, notification, preference, etc.)
-- aggregate_id: aggregate-specific identifier (thread_id, notification_id, "global", etc.)

ALTER TABLE events ADD COLUMN IF NOT EXISTS aggregate TEXT NOT NULL DEFAULT 'thread';
ALTER TABLE events ADD COLUMN IF NOT EXISTS aggregate_id TEXT;

-- Backfill aggregate_id from thread_id for existing thread events
UPDATE events SET aggregate_id = thread_id::text WHERE thread_id IS NOT NULL AND aggregate_id IS NULL;

-- Index for querying events by aggregate + aggregate_id (used by snapshot endpoint, projection rebuild)
CREATE INDEX IF NOT EXISTS idx_events_aggregate_aggregate_id_seq
  ON events (aggregate, aggregate_id, sequence);
