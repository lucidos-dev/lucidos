-- A withdrawn ceiling handoff ends a pending one too (ADR 0629), so the
-- pending-handoff probe reads SessionHandoffWithdrawn rows. The partial index
-- must hold them, or the planner leaves it for a scan of the events table.
-- The type list must match HANDOFF_LIFECYCLE_EVENT_TYPES in engine/successor.rs.
DROP INDEX IF EXISTS idx_events_handoff_lifecycle;
CREATE INDEX idx_events_handoff_lifecycle
  ON events (thread_id, sequence)
  WHERE event_type IN ('SessionHandedOff', 'SessionStarted', 'ResponseCanceled', 'ResponseFailed', 'SessionHandoffWithdrawn');
