-- Remove legacy session lifecycle events that are no longer emitted or consumed.
-- Sessions are now discovered from MessageReceived/ScheduledTaskStarted events.
DELETE FROM events WHERE event_type IN ('SessionStarted', 'SessionEnded', 'SessionTitleUpdated');
