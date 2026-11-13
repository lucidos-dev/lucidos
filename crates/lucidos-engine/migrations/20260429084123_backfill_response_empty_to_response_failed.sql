-- Collapse remaining ResponseEmpty events into ResponseFailed.
-- ResponseEmpty was removed from the codebase in favor of ResponseFailed with
-- a diagnostic error string. The prior migration (20260429042755) already
-- backfilled "Done." engine-fallback rows to ResponseEmpty; this one carries
-- them (and any runtime-emitted ResponseEmpty events) the rest of the way.
--
-- Legacy rows have no stop_reason / output_tokens / thinking_chars captured —
-- the diagnostic fields land on new rows only. The error string flags this so
-- a future reader doesn't mistake a backfilled row for a fresh one.

UPDATE events
SET
    event_type = 'ResponseFailed',
    payload = (payload - 'text' - 'images' - 'model' - 'reasoning_effort')
              || jsonb_build_object(
                   'error',
                   'Model returned no response (legacy "Done." fallback — '
                   || 'no stop_reason / output_tokens / thinking_chars captured)'
                 )
WHERE event_type = 'ResponseEmpty';
