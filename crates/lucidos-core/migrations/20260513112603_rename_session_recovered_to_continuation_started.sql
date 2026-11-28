-- Rename SessionRecovered events to ContinuationStarted.
-- The Rust enum was renamed because (a) "session" was ambiguous between chat
-- and CC, and (b) the resumed work is a *new* response continuing the prior
-- attempt, not the prior response coming back to life. Serde aliases keep
-- old payloads readable, but rewriting the rows lets queries drop the legacy
-- name from their IN-lists at the next cleanup. (Mirrors the earlier
-- SessionResumed → SessionRecovered rewrite from 20260320090000.)
UPDATE events
SET event_type = 'ContinuationStarted',
    payload = jsonb_set(payload, '{type}', '"ContinuationStarted"')
WHERE event_type = 'SessionRecovered';

-- EngineReason::SessionRecovered → EngineReason::ContinuationStarted. Found
-- on `UserPromptInjected.origin.reason.kind` (engine note attribution) and on
-- `ContinuationStarted.origin.reason.kind` (set by the recovery path itself).
-- Walks JSON anywhere it appears under `payload.origin.reason.kind` or
-- `payload.actor.reason.kind` — those are the only places `MessageOrigin`
-- with an Engine variant is persisted today.
UPDATE events
SET payload = jsonb_set(payload, '{origin,reason,kind}', '"continuation_started"')
WHERE payload #>> '{origin,reason,kind}' = 'session_recovered';

UPDATE events
SET payload = jsonb_set(payload, '{actor,reason,kind}', '"continuation_started"')
WHERE payload #>> '{actor,reason,kind}' = 'session_recovered';
