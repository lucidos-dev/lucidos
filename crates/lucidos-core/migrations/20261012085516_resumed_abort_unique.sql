-- An abort is resumed at most once. Two Continue clicks can both find the same
-- unresumed ResponseAborted. Only the first ContinuationStarted claiming it
-- lands, and the second emit fails on this index, so its rerun never spawns.
-- Rows without `resumed_abort_id` (coding-agent, legacy) index as NULL and
-- never collide.
CREATE UNIQUE INDEX IF NOT EXISTS events_resumed_abort_unique
    ON events ((thread_id), ((payload->>'resumed_abort_id')))
    WHERE event_type = 'ContinuationStarted';
