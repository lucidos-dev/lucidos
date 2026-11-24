-- Rename OrphanRecoveryStarted events to SessionResumed
UPDATE events SET event_type = 'SessionResumed' WHERE event_type = 'OrphanRecoveryStarted';
