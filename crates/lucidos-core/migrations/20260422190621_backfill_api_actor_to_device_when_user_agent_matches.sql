-- Backfill historical events whose actor / origin was stamped as `Api { user_agent }`
-- when it should have been `Device { id, label }`.
--
-- Cause: until the desktop/mobile API client started sending `x-cognos-device-id`
-- on every mutating fetch (apply-now, cancel, discard, answer-question, app-capture,
-- mcp/consent, restart), the engine fell back to `Api { user_agent }` for those
-- requests. Re-attributing by matching `user_agent` to a registered device makes
-- the route popover render "👤 You · {device}" for past actions instead of
-- "API client · Mozilla/5.0 ...".
--
-- Resolution: when multiple devices share the same user_agent string (e.g. the
-- user re-registered after clearing browser data, or two browsers run the same
-- Safari version), pick the most recently seen one. This assumes a single human
-- per workspace, which matches CognOS' personal-workspace model. Devices with
-- NULL user_agent are skipped (no way to match).
--
-- Idempotent: subsequent runs find nothing to update because the actor.kind has
-- already changed from "api" to "device".

WITH best_match AS (
    SELECT DISTINCT ON (user_agent)
        user_agent,
        id,
        COALESCE(name, 'Unknown device') AS label
    FROM devices
    WHERE user_agent IS NOT NULL
    ORDER BY user_agent, last_seen_at DESC
)
-- Re-attribute the `actor` field on change-lifecycle and thread-meta events.
UPDATE events e
SET payload = jsonb_set(
    e.payload,
    '{actor}',
    jsonb_build_object('kind', 'device', 'device_id', d.id, 'label', d.label)
)
FROM best_match d
WHERE e.payload->'actor'->>'kind' = 'api'
  AND e.payload->'actor'->>'user_agent' = d.user_agent;

-- Re-attribute the `origin` field on `MessageReceived` and any other event
-- variants that carry the actor under that key.
WITH best_match AS (
    SELECT DISTINCT ON (user_agent)
        user_agent,
        id,
        COALESCE(name, 'Unknown device') AS label
    FROM devices
    WHERE user_agent IS NOT NULL
    ORDER BY user_agent, last_seen_at DESC
)
UPDATE events e
SET payload = jsonb_set(
    e.payload,
    '{origin}',
    jsonb_build_object('kind', 'device', 'device_id', d.id, 'label', d.label)
)
FROM best_match d
WHERE e.payload->'origin'->>'kind' = 'api'
  AND e.payload->'origin'->>'user_agent' = d.user_agent;
