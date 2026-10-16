-- Rename MergeConflictStarted → MergeConflictDetected for consistency with
-- MissingHardeningDetected. The "Detected" naming reflects passive discovery
-- of state; the resolution work that follows is captured by subsequent events
-- (CodingAgentPromptSent, SessionStarted, ChangeApplied/ChangeApplyFailed).
UPDATE events
SET event_type = 'MergeConflictDetected'
WHERE event_type = 'MergeConflictStarted';
