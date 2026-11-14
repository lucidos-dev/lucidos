-- The pending-handoff probe reads only the handoff lifecycle events. Over the
-- whole events table it scanned thousands of rows per idle thread, so the
-- keep-alive query took seconds. This partial index holds those rows alone.
-- The type list must match HANDOFF_LIFECYCLE_EVENT_TYPES in engine/successor.rs.
CREATE INDEX IF NOT EXISTS idx_events_handoff_lifecycle
  ON events (thread_id, sequence)
  WHERE event_type IN ('SessionHandedOff', 'SessionStarted', 'ResponseCanceled', 'ResponseFailed');
