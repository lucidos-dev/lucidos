-- Rename ClaudeCode* event_type values to CodingAgent* and tag each payload
-- with `agent: "claude-code"` so future agents (Codex, etc.) can be distinguished.
-- The wire-format channel string ("claude_code") is intentionally NOT renamed —
-- only event_type values are remapped.
--
-- Single statement so the events table is scanned once. CASE picks the new name;
-- jsonb_set runs unconditionally (it's a no-op write but the WHERE clause already
-- restricts us to rows that need rewriting).
UPDATE events
   SET event_type = CASE
           WHEN event_type = 'CCSettingsChanged' THEN 'CodingAgentSettingsChanged'
           WHEN event_type = 'CcThreadSpawned'   THEN 'CodingAgentThreadSpawned'
           ELSE 'CodingAgent' || substring(event_type FROM 11)
       END,
       payload = jsonb_set(payload, '{agent}', '"claude-code"', true)
 WHERE event_type LIKE 'ClaudeCode%'
    OR event_type IN ('CCSettingsChanged', 'CcThreadSpawned');
