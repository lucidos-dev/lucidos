-- Rename SessionResumed events to SessionRecovered.
-- SessionRecovered is only emitted for auto-recovery of interrupted sessions.
-- Idle session follow-ups now use SessionStarted (no special event).
UPDATE events
SET event_type = 'SessionRecovered',
    payload = jsonb_set(payload, '{type}', '"SessionRecovered"')
WHERE event_type = 'SessionResumed';
