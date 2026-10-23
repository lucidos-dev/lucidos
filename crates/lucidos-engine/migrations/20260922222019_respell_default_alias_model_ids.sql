-- Re-spell the `@default` chat-model ids to bare first-party ids.
--
-- `@default` is a Vertex-only alias. It survived in four rows because Opus 5
-- and Opus 4.8 were first published in the dated-snapshot era; Sonnet 5 and
-- Opus 5.5 are already seeded bare. Now that a row is served by whichever
-- provider is configured, a Vertex-only spelling as the row's IDENTITY is
-- wrong: the direct Anthropic API rejects it outright.
--
-- Vertex accepts the bare ids. Verified by live probe against `europe-west1`:
-- `claude-opus-5` and `claude-opus-4-8` both resolve (HTTP 429, quota), while a
-- deliberately unknown id answers 404 `Publisher model ... was not found`.
--
-- The four ids, and what they become:
--   claude-opus-5@default       -> claude-opus-5
--   claude-opus-5@default[1m]   -> claude-opus-5[1m]
--   claude-opus-4-8@default     -> claude-opus-4-8
--   claude-opus-4-8@default[1m] -> claude-opus-4-8[1m]
--
-- `claude-opus-4-5@20251101` is deliberately untouched. It is a dated Vertex
-- snapshot, not a `@default` alias, and it has no bare equivalent.
--
-- See docs/plans/2026-09-22-one-model-any-configured-provider.md.

-- The mapping, as a table the statements below join against. A temp table
-- rather than four CASE arms repeated per store, so no store can drift.
CREATE TEMP TABLE respelled_model_ids (old TEXT PRIMARY KEY, new TEXT NOT NULL)
ON COMMIT DROP;
INSERT INTO respelled_model_ids (old, new) VALUES
  ('claude-opus-5@default',       'claude-opus-5'),
  ('claude-opus-5@default[1m]',   'claude-opus-5[1m]'),
  ('claude-opus-4-8@default',     'claude-opus-4-8'),
  ('claude-opus-4-8@default[1m]', 'claude-opus-4-8[1m]');

-- 1. The registry rows themselves.
--
-- Guarded on the target being free. A workspace that already hand-added
-- `claude-opus-5` would otherwise collide on the primary key and fail the boot.
-- Leaving the legacy row in place is the safe side: it still routes, and the
-- user can merge the two by hand.
UPDATE models AS m
SET id = r.new, updated_at = NOW()
FROM respelled_model_ids AS r
WHERE m.id = r.old
  AND NOT EXISTS (SELECT 1 FROM models AS other WHERE other.id = r.new);

-- A renamed Opus 5 row now carries an id the direct Anthropic API accepts, so
-- it gains that route. The routes migration left it out while the id was still
-- the Vertex-only alias. A builtin only: a user row under the bare id is theirs.
UPDATE models
SET routes = routes || '[{"provider": "anthropic"}]'::jsonb,
    updated_at = NOW()
WHERE source = 'builtin'
  AND id IN ('claude-opus-5', 'claude-opus-5[1m]')
  AND NOT EXISTS (
        SELECT 1 FROM jsonb_array_elements(routes) AS r
         WHERE r ->> 'provider' = 'anthropic'
      );

-- A rename the guard above skipped leaves the legacy row in place, and the
-- bare id then names somebody else's row. Its references must stay where they
-- are, so drop those pairs before any store is rewritten.
DELETE FROM respelled_model_ids AS r
WHERE EXISTS (SELECT 1 FROM models AS m WHERE m.id = r.old);

-- 2. Every saved preference naming one: `chat_model` and the auxiliary
-- `model_*` keys, global and any device-scoped copy. Matching on the exact
-- value is safe, since no other preference holds a string spelled like these.
UPDATE preferences AS p
SET value = r.new
FROM respelled_model_ids AS r
WHERE p.value = r.old;

-- 3. Per-draft compose selections, which carry the draft's own model pick.
UPDATE thread_summaries AS t
SET compose_selection = jsonb_set(t.compose_selection, '{model}', to_jsonb(r.new))
FROM respelled_model_ids AS r
WHERE t.compose_selection ->> 'model' = r.old;

-- 4. Turns waiting in the Thread Queue, which carry their model for when they
-- are admitted.
UPDATE thread_queue AS q
SET request = jsonb_set(q.request, '{model}', to_jsonb(r.new))
FROM respelled_model_ids AS r
WHERE q.request ->> 'model' = r.old;

-- 5. Event payloads.
--
-- Events are immutable and append-only, and this is a deliberate one-shot
-- exception, recorded in docs/adr/. It follows the precedent set by
-- 20260416080000_migrate_model_aliases_to_full_ids.sql, for the same reason:
-- these rows are READ BACK AS A LIVE SETTING, not only as history.
--
-- `MessageReceived` and `TriggerStarted` are what `last_thread_chat_settings`
-- reads for per-thread model memory. Leave them and a thread's remembered model
-- names a row that no longer exists, so it silently falls to the id-shape guess.
-- The three response events are what the transcript renders a past turn's model
-- from, and an unlisted id shows up as a phantom entry in the picker.
-- `TriggerCreated` and `TriggerUpdated` carry a trigger's pinned model.
--
-- Deliberately NOT rewritten, because they are records of what was actually
-- spent or sent rather than settings read back:
--   * `ContextCaptured` and `ContextAssembled` (the cost ledger the Token Cost
--     app reads; the request really did name `@default`).
--   * `CodingAgentSettingsChanged` (Claude Code's own id vocabulary, from
--     `runtime/cc_menu_options.json`, where `claude-opus-5@default` is still
--     the correct spelling).
--   * `JevCallCompleted`, `ImageDescribed`, `ConversationSummarized` (other
--     model namespaces, none of them a chat-registry id).
UPDATE events AS e
SET payload = jsonb_set(e.payload, '{model}', to_jsonb(r.new))
FROM respelled_model_ids AS r
WHERE e.event_type IN (
        'MessageReceived',
        'TriggerStarted',
        'ResponseGenerated',
        'ResponseCanceled',
        'ResponseAborted',
        'TriggerCreated',
        'TriggerUpdated'
      )
  AND e.payload ->> 'model' = r.old;
