-- Retire the unattended permission wait.
--
-- Every coding-agent session now takes the workspace's grants and a card that
-- waits for a human, so no card parks on a timer and
-- `unattended_permission_wait` no longer means anything. A stored row would
-- only be dead data. A trigger's own `permission_wait` lives in its events,
-- which stay as written: replay ignores the field.
DELETE FROM preferences WHERE key = 'unattended_permission_wait';
