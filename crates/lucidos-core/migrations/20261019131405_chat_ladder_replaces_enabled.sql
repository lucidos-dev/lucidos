-- The chat intelligence ladder replaces the registry's `enabled` switch
-- (ADR 0525). See docs/plans/2026-10-19-chat-intelligence-ladder.md, phase 5.
--
-- Upgrading must hide nothing from the chat picker. So this snapshots the
-- models a workspace had switched on, and the engine turns them into the
-- `intelligence_ladder_chat` override at its next boot. Rust does that half:
-- the rung order and each model's tiers live in the rung estimates and the
-- router, which SQL cannot read. The engine empties the table once it has
-- run, so the upgrade happens once.
--
-- A new database has no events yet, so it snapshots nothing and keeps the
-- shipped chat ladder. The table goes with the `enabled` column.
CREATE TABLE IF NOT EXISTS chat_ladder_upgrade_models (
    model_id TEXT PRIMARY KEY
);

INSERT INTO chat_ladder_upgrade_models (model_id)
SELECT id FROM models
 WHERE enabled
   AND successor IS NULL
   AND EXISTS (SELECT 1 FROM events)
ON CONFLICT (model_id) DO NOTHING;

-- A chat ladder's pairs with `old_id` moved to `successor_id` at a tier it
-- takes. Unlike `retire_model_in_pairs`, each tier is its own rung, so a
-- moved pair survives beside the successor's other rungs. It is dropped only
-- when the ladder already holds the rung it would become, since the override
-- refuses a rung named twice.
CREATE OR REPLACE FUNCTION retire_model_in_ladder(
  list TEXT, old_id TEXT, successor_id TEXT, accepted TEXT[]
) RETURNS TEXT
LANGUAGE sql IMMUTABLE AS $$
  WITH items AS (
    SELECT n, item,
           strpos(item, '=') > 0 AND btrim(split_part(item, '=', 1)) = old_id AS names_old
      FROM regexp_split_to_table(list, ',') WITH ORDINALITY AS pairs (item, n)
  ), moved AS (
    SELECT n, names_old,
           CASE
             WHEN names_old
               THEN substring(item FROM '^\s*') || successor_id || '='
                    || snap_effort(btrim(substr(item, strpos(item, '=') + 1)), accepted)
             ELSE item
           END AS item
      FROM items
  ), keyed AS (
    SELECT n, names_old, item,
           btrim(split_part(item, '=', 1)) || '='
             || btrim(substr(item, strpos(item, '=') + 1)) AS rung
      FROM moved
  )
  SELECT string_agg(item, ',' ORDER BY n)
    FROM keyed AS k
   WHERE NOT (k.names_old AND EXISTS (
           SELECT 1 FROM keyed AS other
            WHERE other.rung = k.rung
              AND (NOT other.names_old OR other.n < k.n)))
$$;

-- As before, except the chat ladder moves through `retire_model_in_ladder`,
-- and the retired row's `enabled` is no longer read or written.
CREATE OR REPLACE FUNCTION retire_model(
  old_id TEXT, successor_id TEXT, accepted TEXT[]
) RETURNS VOID
LANGUAGE plpgsql AS $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM models WHERE id = successor_id) THEN
    RAISE EXCEPTION 'retire_model: successor % has no models row', successor_id;
  END IF;

  UPDATE models
     SET successor = successor_id, updated_at = NOW()
   WHERE id = old_id
     AND successor IS DISTINCT FROM successor_id;

  -- The tier first: it is found through the model key, which moves next.
  UPDATE preferences AS tier
     SET value = snap_effort(tier.value, accepted), updated_at = NOW()
   WHERE tier.value IS DISTINCT FROM snap_effort(tier.value, accepted)
     AND EXISTS (
           SELECT 1 FROM preferences AS model
            WHERE model.value = old_id
              AND model.key LIKE 'model\_%'
              AND NOT is_coding_agent_setting(model.key)
              AND tier.key = 'reasoning_' || substr(model.key, length('model_') + 1)
         );

  UPDATE preferences
     SET value = successor_id, updated_at = NOW()
   WHERE value = old_id
     AND (key = 'chat_model' OR key LIKE 'model\_%')
     AND NOT is_coding_agent_setting(key);

  UPDATE preferences
     SET value = retire_model_in_ladder(value, old_id, successor_id, accepted),
         updated_at = NOW()
   WHERE key = 'intelligence_ladder_chat'
     AND pairs_name_model(value, old_id);

  UPDATE preferences
     SET value = retire_model_in_pairs(value, old_id, successor_id, accepted),
         updated_at = NOW()
   WHERE pairs_name_model(value, old_id)
     AND key <> 'intelligence_ladder_chat'
     AND NOT is_coding_agent_setting(key);

  UPDATE thread_summaries
     SET compose_selection = retire_model_in_json(
           compose_selection, old_id, successor_id, 'reasoningEffort', accepted)
   WHERE compose_selection ->> 'model' = old_id
     AND NOT is_coding_agent;

  UPDATE thread_queue
     SET request = retire_model_in_json(
           request, old_id, successor_id, 'reasoning_effort', accepted)
   WHERE request ->> 'model' = old_id
     AND request ->> 'type' IS DISTINCT FROM 'coding-agent';

  UPDATE events
     SET payload = retire_model_in_json(
           payload, old_id, successor_id, 'reasoning_effort', accepted)
   WHERE event_type IN ('MessageReceived', 'TriggerStarted', 'TriggerCreated', 'TriggerUpdated')
     AND payload ->> 'model' = old_id
     AND NOT EXISTS (
           SELECT 1 FROM thread_summaries AS t
            WHERE events.aggregate = 'thread'
              AND t.thread_id::text = events.aggregate_id
              AND t.is_coding_agent
         );
END
$$;
