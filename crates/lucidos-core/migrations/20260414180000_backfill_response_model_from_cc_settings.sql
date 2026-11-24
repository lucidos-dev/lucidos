-- Backfill model on ResponseGenerated/ResponseCanceled/ResponseAborted events
-- that have no model set, using the most recent CCSettingsChanged event with a
-- model in the same thread (before the response event).
--
-- This covers CC sessions where the user explicitly selected a model but the
-- engine didn't propagate it to ResponseGenerated. Sessions using the default
-- model (no CCSettingsChanged with model) remain unset — no data to deduce from.

UPDATE events r
SET payload = jsonb_set(r.payload, '{model}', to_jsonb(settings.model))
FROM (
    SELECT DISTINCT ON (r2.id)
        r2.id AS response_id,
        s.payload->>'model' AS model
    FROM events r2
    JOIN events s
      ON s.aggregate_id = r2.aggregate_id
     AND s.event_type = 'CCSettingsChanged'
     AND s.payload->>'model' IS NOT NULL
     AND s.sequence < r2.sequence
    WHERE r2.event_type IN ('ResponseGenerated', 'ResponseCanceled', 'ResponseAborted')
      AND (r2.payload->>'model') IS NULL
    ORDER BY r2.id, s.sequence DESC
) settings
WHERE r.id = settings.response_id;
