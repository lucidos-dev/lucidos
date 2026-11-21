-- Retire a model to its successor, and retire Gemini 3.5 Flash to 3.8 Flash.
--
-- A provider that retires a model often routes its old id to a newer one under
-- the hood. Lucidos never relies on that: the user did not choose that model,
-- and the price it bills is not stated. So a retired model is MIGRATED, on
-- every install, by `retire_model` below. ADR 0418 records the rule and why.
--
-- The next deprecation is one line in a new migration:
--   SELECT retire_model('<old id>', '<successor id>', ARRAY[<accepted tiers>]);
-- The third argument lists the reasoning tiers EVERY route of the successor
-- accepts (`llm::reasoning::supported_efforts`). A test pins each call's list.

-- The successor of a retired model. NULL means the model is not retired. The
-- router follows it, so a retired id that still reaches a call (an env var, a
-- CLI write, a plugin's trigger file) is sent as its successor.
ALTER TABLE models
  ADD COLUMN IF NOT EXISTS successor TEXT REFERENCES models (id) ON DELETE SET NULL;
ALTER TABLE models DROP CONSTRAINT IF EXISTS models_successor_is_another_row;
ALTER TABLE models
  ADD CONSTRAINT models_successor_is_another_row CHECK (successor IS DISTINCT FROM id);

-- `effort` moved onto the nearest tier in `accepted`, the higher one winning a
-- tie. This mirrors `llm::reasoning::clamp_effort`, and a test holds the two
-- equal over the whole ladder. A value that is not a tier (a byte cap, a
-- typo) comes back unchanged.
CREATE OR REPLACE FUNCTION snap_effort(effort TEXT, accepted TEXT[]) RETURNS TEXT
LANGUAGE sql IMMUTABLE AS $$
  WITH ladder (tiers) AS (
    SELECT ARRAY['none', 'low', 'medium', 'high', 'xhigh', 'max']
  )
  SELECT CASE
    WHEN effort IS NULL
      OR array_position(tiers, effort) IS NULL
      OR effort = ANY (accepted) THEN effort
    ELSE (
      SELECT tier FROM unnest(accepted) AS tier
       WHERE array_position(tiers, tier) IS NOT NULL
       ORDER BY abs(array_position(tiers, tier) - array_position(tiers, effort)),
                array_position(tiers, tier) DESC
       LIMIT 1
    )
  END
  FROM ladder
$$;

-- A `model=value, ...` preference list with `old_id`'s pairs renamed to
-- `successor_id` and their value snapped. `parse_model_pairs` splits on `,`,
-- then on the first `=`, and trims both halves; this reads it the same way.
-- Every other pair passes through byte for byte.
CREATE OR REPLACE FUNCTION retire_model_in_pairs(
  list TEXT, old_id TEXT, successor_id TEXT, accepted TEXT[]
) RETURNS TEXT
LANGUAGE sql IMMUTABLE AS $$
  SELECT string_agg(
    CASE
      WHEN strpos(item, '=') > 0 AND btrim(split_part(item, '=', 1)) = old_id
        THEN substring(item FROM '^\s*') || successor_id || '='
             || snap_effort(btrim(substr(item, strpos(item, '=') + 1)), accepted)
      ELSE item
    END,
    ',' ORDER BY n
  )
  FROM regexp_split_to_table(list, ',') WITH ORDINALITY AS pairs (item, n)
$$;

-- Whether a `model=value, ...` list names `old_id` as a model.
CREATE OR REPLACE FUNCTION pairs_name_model(list TEXT, old_id TEXT) RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE AS $$
  SELECT EXISTS (
    SELECT 1 FROM regexp_split_to_table(list, ',') AS item
     WHERE strpos(item, '=') > 0 AND btrim(split_part(item, '=', 1)) = old_id
  )
$$;

-- A JSON document naming `old_id` at its top-level `model`, moved to
-- `successor_id`, with the tier under `effort_key` snapped when it is a string.
CREATE OR REPLACE FUNCTION retire_model_in_json(
  doc JSONB, old_id TEXT, successor_id TEXT, effort_key TEXT, accepted TEXT[]
) RETURNS JSONB
LANGUAGE sql IMMUTABLE AS $$
  SELECT CASE
    WHEN doc ->> 'model' IS DISTINCT FROM old_id THEN doc
    WHEN jsonb_typeof(doc -> effort_key) = 'string'
      THEN jsonb_set(doc, '{model}', to_jsonb(successor_id))
           || jsonb_build_object(effort_key, snap_effort(doc ->> effort_key, accepted))
    ELSE jsonb_set(doc, '{model}', to_jsonb(successor_id))
  END
$$;

-- Retire `old_id`: disable its row, name its successor, and move every saved
-- setting that names it to `successor_id`. Re-running it changes nothing.
--
-- Rewritten, because each is READ BACK AS A LIVE SETTING (ADR 0248's test):
--   * every preference holding exactly the id (`chat_model`, `model_*`), and
--     the `reasoning_<purpose>` tier beside a rewritten `model_<purpose>`;
--   * `model=value` lists (`chat_reasoning_efforts`, `memory_view_model_caps`);
--   * draft compose selections and turns waiting in the Thread Queue;
--   * `MessageReceived` and `TriggerStarted` (per-thread model memory), and
--     `TriggerCreated` and `TriggerUpdated` (a trigger's pinned model).
--
-- Deliberately NOT rewritten: response events, `ContextCaptured` and every
-- other record of which model answered or what it cost. The old model really
-- did answer those turns.
CREATE OR REPLACE FUNCTION retire_model(
  old_id TEXT, successor_id TEXT, accepted TEXT[]
) RETURNS VOID
LANGUAGE plpgsql AS $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM models WHERE id = successor_id) THEN
    RAISE EXCEPTION 'retire_model: successor % has no models row', successor_id;
  END IF;

  UPDATE models
     SET enabled = FALSE, successor = successor_id, updated_at = NOW()
   WHERE id = old_id
     AND (enabled OR successor IS DISTINCT FROM successor_id);

  -- The tier first: it is found through the model key, which moves next.
  UPDATE preferences AS tier
     SET value = snap_effort(tier.value, accepted), updated_at = NOW()
   WHERE tier.value IS DISTINCT FROM snap_effort(tier.value, accepted)
     AND EXISTS (
           SELECT 1 FROM preferences AS model
            WHERE model.value = old_id
              AND model.key LIKE 'model\_%'
              AND tier.key = 'reasoning_' || substr(model.key, length('model_') + 1)
         );

  UPDATE preferences
     SET value = successor_id, updated_at = NOW()
   WHERE value = old_id
     AND (key = 'chat_model' OR key LIKE 'model\_%');

  UPDATE preferences
     SET value = retire_model_in_pairs(value, old_id, successor_id, accepted),
         updated_at = NOW()
   WHERE pairs_name_model(value, old_id);

  UPDATE thread_summaries
     SET compose_selection = retire_model_in_json(
           compose_selection, old_id, successor_id, 'reasoningEffort', accepted)
   WHERE compose_selection ->> 'model' = old_id;

  UPDATE thread_queue
     SET request = retire_model_in_json(
           request, old_id, successor_id, 'reasoning_effort', accepted)
   WHERE request ->> 'model' = old_id;

  UPDATE events
     SET payload = retire_model_in_json(
           payload, old_id, successor_id, 'reasoning_effort', accepted)
   WHERE event_type IN ('MessageReceived', 'TriggerStarted', 'TriggerCreated', 'TriggerUpdated')
     AND payload ->> 'model' = old_id;
END
$$;

-- Google deprecated Gemini 3.5 Flash and routes its id to 3.6 Flash. 3.8 Flash
-- is the successor: newer, the same price, and already registered with a
-- Vertex and an OpenRouter route. It always reasons, so it takes no `none`.
SELECT retire_model('gemini-3.5-flash', 'gemini-3.8-flash', ARRAY['low', 'medium', 'high']);
