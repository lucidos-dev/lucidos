-- Seed the two newest small models, and retire every superseded model to its
-- successor. See docs/plans/2026-10-18-model-defaults-audit.md.
--
-- GPT-6 Luna succeeds GPT-5.4 mini and GPT-5.6 Luna at half the price. Gemini
-- 3.1 Flash-Lite succeeds Gemini 2.5 Flash-Lite, which Vertex retired. Both
-- head background fallbacks, so each needs an OpenRouter route beside its
-- first-party one, first party first.
--
-- Windows: GPT-6 Luna 1,050,000 tokens and Gemini 3.1 Flash-Lite 1,048,576,
-- from the providers' model pages and OpenRouter's listings. Both read images.
-- Default effort: OpenAI documents `medium` for GPT-6 Luna. Gemini 3.1
-- Flash-Lite defaults to `minimal` thinking, and `low` is the nearest tier.
--
-- sort_order joins each family's neighbours: 36 beside GPT-6.1 Sol and GPT-6
-- Astra, 33 after Gemini 3 Flash. Builtin = disable-only, enabled by default.
-- ON CONFLICT DO NOTHING leaves a user-created row under either id untouched.
INSERT INTO models (id, label, routes, sort_order, source, vision, default_effort) VALUES
  ('gpt-6-luna', 'GPT-6 Luna',
   '[{"provider": "openai", "context_window": 1050000},
     {"provider": "openrouter", "id": "openai/gpt-6-luna", "context_window": 1050000}]'::jsonb,
   36, 'builtin', TRUE, 'medium'),
  ('gemini-3.1-flash-lite', 'Gemini 3.1 Flash-Lite',
   '[{"provider": "vertex", "context_window": 1048576},
     {"provider": "openrouter", "id": "google/gemini-3.1-flash-lite", "context_window": 1048576}]'::jsonb,
   33, 'builtin', TRUE, 'low')
ON CONFLICT (id) DO NOTHING;

-- A coding agent's model comes from that agent's own lineup, not from this
-- registry. Claude Code and Codex still offer ids retired below, and a stored
-- pick they stop offering already reads as no default.
CREATE OR REPLACE FUNCTION is_coding_agent_setting(key TEXT) RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE AS $$
  SELECT key IN ('model_claude_code', 'model_codex',
                 'intelligence_ladder_claude_code', 'intelligence_ladder_codex')
$$;

-- As before, except a list that already names the successor drops the
-- retired pair. Several models now retire to one successor, and a renamed
-- pair would duplicate the user's own entry for it and win the lookup.
CREATE OR REPLACE FUNCTION retire_model_in_pairs(
  list TEXT, old_id TEXT, successor_id TEXT, accepted TEXT[]
) RETURNS TEXT
LANGUAGE sql IMMUTABLE AS $$
  SELECT string_agg(
    CASE
      WHEN names_old
        THEN substring(item FROM '^\s*') || successor_id || '='
             || snap_effort(btrim(substr(item, strpos(item, '=') + 1)), accepted)
      ELSE item
    END,
    ',' ORDER BY n
  )
  FROM regexp_split_to_table(list, ',') WITH ORDINALITY AS pairs (item, n),
       LATERAL (SELECT strpos(item, '=') > 0
                       AND btrim(split_part(item, '=', 1)) = old_id AS names_old) AS old
  WHERE NOT (names_old AND pairs_name_model(list, successor_id))
$$;

-- As before, except it leaves every coding-agent setting alone: the four
-- preferences above with their tiers, coding-agent drafts and queued spawns,
-- and the turns of coding-agent threads.
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
              AND NOT is_coding_agent_setting(model.key)
              AND tier.key = 'reasoning_' || substr(model.key, length('model_') + 1)
         );

  UPDATE preferences
     SET value = successor_id, updated_at = NOW()
   WHERE value = old_id
     AND (key = 'chat_model' OR key LIKE 'model\_%')
     AND NOT is_coding_agent_setting(key);

  UPDATE preferences
     SET value = retire_model_in_pairs(value, old_id, successor_id, accepted),
         updated_at = NOW()
   WHERE pairs_name_model(value, old_id)
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

-- Each tier list is what EVERY route of the successor accepts: the
-- intersection of `llm::reasoning::supported_efforts` over its routes. A test
-- pins each list. A successor is never itself retired, so no chain forms.

-- The Claude 5 generation, superseded by 5.5 and Fable 5.1. Each successor
-- always thinks, so none takes `none`.
SELECT retire_model('claude-opus-5', 'claude-opus-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-opus-5[1m]', 'claude-opus-5-5[1m]', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-sonnet-5', 'claude-sonnet-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-sonnet-5[1m]', 'claude-sonnet-5-5[1m]', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-fable-5', 'claude-fable-5-1', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-fable-5[1m]', 'claude-fable-5-1[1m]', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);

-- Rows the prior-generation prune switched off with no successor, so an old
-- stored pick lands somewhere current.
SELECT retire_model('claude-opus-4-8', 'claude-opus-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-opus-4-8[1m]', 'claude-opus-5-5[1m]', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-opus-4-7', 'claude-opus-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-opus-4-7[1m]', 'claude-opus-5-5[1m]', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-opus-4-6', 'claude-opus-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-opus-4-6[1m]', 'claude-opus-5-5[1m]', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-opus-4-5@20251101', 'claude-opus-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-sonnet-4-6', 'claude-sonnet-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-sonnet-4-6[1m]', 'claude-sonnet-5-5[1m]', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);

-- Haiku 4.5 is past its retirement floor on the Claude API and on Vertex.
-- Haiku 5.5's OpenRouter route tops out at `high`.
SELECT retire_model('claude-haiku-4-5', 'claude-haiku-5-5', ARRAY['none', 'low', 'medium', 'high']);

-- Google points from the Gemini 3 Flash preview to newer Flash.
SELECT retire_model('gemini-3-flash-preview', 'gemini-3.8-flash', ARRAY['low', 'medium', 'high']);

-- OpenAI's small models. GPT-6 Luna's OpenRouter route tops out at `high`.
SELECT retire_model('gpt-5.4-mini', 'gpt-6-luna', ARRAY['none', 'low', 'medium', 'high']);
SELECT retire_model('gpt-5.6-luna', 'gpt-6-luna', ARRAY['none', 'low', 'medium', 'high']);

-- OpenAI's mid tier. GPT-6.1 Sol always reasons, and its OpenRouter route
-- tops out at `high`.
SELECT retire_model('gpt-5.6-sol', 'gpt-6.1-sol', ARRAY['low', 'medium', 'high']);
SELECT retire_model('gpt-5.6-terra', 'gpt-6.1-sol', ARRAY['low', 'medium', 'high']);
SELECT retire_model('gpt-5.5', 'gpt-6.1-sol', ARRAY['low', 'medium', 'high']);
SELECT retire_model('gpt-5.4', 'gpt-6.1-sol', ARRAY['low', 'medium', 'high']);

-- OpenAI's top tier.
SELECT retire_model('gpt-5.5-pro', 'gpt-6-astra', ARRAY['none', 'low', 'medium', 'high', 'xhigh', 'max']);
