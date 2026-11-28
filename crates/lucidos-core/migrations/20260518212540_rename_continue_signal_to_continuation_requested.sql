-- Rename the old `ContinueSignal` event_type to `ContinuationRequested`.
--
-- The Rust enum was renamed with `#[serde(alias = "ContinueSignal")]` so the
-- JSON payload would still deserialize, but the `event_type` text column is
-- queried directly in several places (SSE filtering, scheduler matcher,
-- projection rebuilds, status_transitions string matches) that bypass the
-- typed enum. Normalize at rest so every consumer sees the canonical name.
--
-- Updates both the column and the JSON payload's `type` discriminator so the
-- two stay in sync.

UPDATE events
SET
    event_type = 'ContinuationRequested',
    payload = jsonb_set(payload, '{type}', '"ContinuationRequested"')
WHERE event_type = 'ContinueSignal';
