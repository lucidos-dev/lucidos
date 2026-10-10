-- Repair notification taps whose thread id is the `current` / `this` alias.
--
-- Producers used to store `tap.to.id` verbatim, so an agent writing the alias
-- it knows from the `events` tool left a deep link the page cannot resolve.
-- Tapping such a row raises `Thread "current" no longer exists`. The engine now
-- settles that id at every producer; this repairs the rows already written.
-- Full reasoning:
-- docs/plans/2026-09-18-notification-tap-thread-id-is-a-uuid.md
--
-- The row's own `thread_id` column is the value the tap should have carried:
-- the engine stamps it from the originating thread, never from model output.
-- A row without one falls back to the card, which is what `default_tap` would
-- have produced for it.
--
-- Scoped to the two alias words, NOT to "anything that fails a uuid regex".
-- `Uuid::parse_str` also accepts the unhyphenated, braced and urn forms, so a
-- regex narrower than the parser would read a WORKING deep link as broken and
-- overwrite it with a different thread. A tap silently pointing at the wrong
-- conversation is worse than the error, which at least announces itself. No
-- word is a uuid in any form, so this direction destroys nothing.

UPDATE notifications
SET tap = jsonb_set(tap, '{to,id}', to_jsonb(thread_id::text))
WHERE tap->>'kind' = 'navigate'
  AND tap->'to'->>'target' = 'thread'
  AND lower(btrim(tap->'to'->>'id')) IN ('current', 'this')
  AND thread_id IS NOT NULL;

UPDATE notifications
SET tap = '{"kind":"modal"}'::jsonb
WHERE tap->>'kind' = 'navigate'
  AND tap->'to'->>'target' = 'thread'
  AND lower(btrim(tap->'to'->>'id')) IN ('current', 'this')
  AND thread_id IS NULL;
