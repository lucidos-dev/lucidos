-- A held message is released at most once. An answer and a new human message
-- can both try to release the same held messages, and each release reads, then
-- emits. This index makes the second HeldMessageReleased fail at the DB, so
-- the loser skips that message instead of delivering it twice. See ADR 0256.
CREATE UNIQUE INDEX IF NOT EXISTS events_held_message_released_unique
    ON events ((thread_id), ((payload->>'held_message_id')))
    WHERE event_type = 'HeldMessageReleased';
