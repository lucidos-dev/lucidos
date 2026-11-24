-- Backfill historical engine-fallback "Done." rows to ResponseEmpty.
-- A row is engine-fallback iff event_type=ResponseGenerated, text='Done.', and
-- no TextStreamed event preceded it within 60s (no streamed output → the
-- agentic loop fell through to the now-removed unwrap_or_else fallback).
-- Rows preceded by TextStreamed: "Done." are preserved (real model output).

UPDATE events
SET
  event_type = 'ResponseEmpty',
  payload = payload - 'text' - 'images'
WHERE event_type = 'ResponseGenerated'
  AND payload->>'text' = 'Done.'
  AND NOT EXISTS (
    SELECT 1 FROM events e2
    WHERE e2.aggregate_id = events.aggregate_id
      AND e2.created BETWEEN events.created - interval '60 seconds' AND events.created
      AND e2.event_type = 'TextStreamed'
  );
