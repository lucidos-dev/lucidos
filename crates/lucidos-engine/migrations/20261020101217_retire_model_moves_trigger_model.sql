-- As before, except the `trigger_model` default moves with the chat one
-- when its model retires.
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
     AND (key IN ('chat_model', 'trigger_model') OR key LIKE 'model\_%')
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
