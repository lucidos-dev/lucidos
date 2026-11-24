-- Coding-agent auto-answer splits off the command guard, so
-- 'command_guard_coding_agents' becomes 'coding_agent_auto_answer'
-- (docs/plans/2026-11-10-command-guard-on-by-default.md).

-- The old key was read only under a live guard, so a stored 'true' under an
-- off or unset guard did nothing. It carries over as what it did: off.
UPDATE preferences SET value = 'false'
  WHERE key = 'command_guard_coding_agents' AND value = 'true'
    AND NOT EXISTS (
      SELECT 1 FROM preferences
       WHERE key = 'command_guard' AND value = 'true' AND device_id IS NULL
    );

-- A row already under the new key on the same device wins over the old one.
DELETE FROM preferences f
  USING preferences t
  WHERE f.key = 'command_guard_coding_agents'
    AND t.key = 'coding_agent_auto_answer'
    AND COALESCE(f.device_id, '') = COALESCE(t.device_id, '');
UPDATE preferences SET key = 'coding_agent_auto_answer'
  WHERE key = 'command_guard_coding_agents';

-- The old key defaulted on under a live guard, and the new one defaults off.
-- A guarded workspace that never stored a choice auto-answers today, so it
-- stores that. Every other workspace keeps its row or reads the default.
INSERT INTO preferences (key, value, device_id)
SELECT 'coding_agent_auto_answer', 'true', NULL
WHERE EXISTS (
  SELECT 1 FROM preferences
   WHERE key = 'command_guard' AND value = 'true' AND device_id IS NULL
)
ON CONFLICT (key, COALESCE(device_id, '')) DO NOTHING;
