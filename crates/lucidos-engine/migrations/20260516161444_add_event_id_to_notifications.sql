-- Add event_id (optional) to notifications so a notification can deep-link to
-- a specific event inside its thread (e.g. the UserQuestionAsked or
-- CodingAgentPermissionRequest the user should jump straight to). Nullable —
-- most notifications won't have a target event (cron summaries, standalone
-- messages). Existing rows are backfilled to NULL.
ALTER TABLE notifications ADD COLUMN event_id UUID;
