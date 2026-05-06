-- Backfill has_changes on ClaudeCodeIdled events that predate the field.
-- For each thread with a ClaudeCodeIdled event missing has_changes,
-- check if the corresponding SessionStarted branch still has a worktree
-- with changes. Since we can't check git from SQL, default to true —
-- if a CC session went idle, it almost certainly produced work.
-- The user can click "End Session" if there are genuinely no changes.

UPDATE events
SET payload = payload || '{"has_changes": true}'::jsonb
WHERE event_type = 'ClaudeCodeIdled'
  AND (payload->>'has_changes') IS NULL;
