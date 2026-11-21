-- Backfill MessageReceived payloads from legacy `sender` field to `mode`.
--
-- Pre-refactor rows used sender: "user" | "system". The deserializer aliased
-- those to ActorMode::Human / Agent until commit 6d7d2fd2 dropped the alias.
-- This migration converts payloads in-place so the rows continue to load as
-- their original semantics (system -> agent, user -> human), then removes the
-- now-redundant `sender` key.

-- 1. system -> agent
UPDATE events
SET payload = jsonb_set(payload - 'sender', '{mode}', '"agent"')
WHERE event_type = 'MessageReceived'
  AND payload->>'sender' = 'system'
  AND NOT (payload ? 'mode');

-- 2. user -> human
UPDATE events
SET payload = jsonb_set(payload - 'sender', '{mode}', '"human"')
WHERE event_type = 'MessageReceived'
  AND payload->>'sender' = 'user'
  AND NOT (payload ? 'mode');

-- 3. Strip any remaining `sender` keys (rows where both mode AND sender were set).
UPDATE events
SET payload = payload - 'sender'
WHERE event_type = 'MessageReceived'
  AND payload ? 'sender';
