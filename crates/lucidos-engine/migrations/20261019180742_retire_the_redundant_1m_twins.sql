-- Retire the `[1m]` twins of the models that only run at 1M. See
-- docs/plans/2026-10-19-retire-redundant-1m-twins.md.
--
-- Fable 5.1, Opus 5.5 and Sonnet 5.5 have no 200k variant, so a twin only adds
-- the 1M beta header, which these models ignore. Each retires to its bare row.

-- retire_model raises on a missing successor, so seed each bare row first, as
-- the earlier seeds and route migrations leave it. ON CONFLICT DO NOTHING
-- leaves an existing row, builtin or user, untouched.
INSERT INTO models (id, label, routes, sort_order, source, vision, default_effort) VALUES
  ('claude-fable-5-1', 'Fable 5.1',
   '[{"provider": "anthropic", "context_window": 1000000},
     {"provider": "claude-gateway", "context_window": 1000000}]'::jsonb,
   -2, 'builtin', TRUE, 'high'),
  ('claude-opus-5-5', 'Opus 5.5',
   '[{"provider": "vertex", "context_window": 1000000},
     {"provider": "anthropic", "context_window": 1000000},
     {"provider": "claude-gateway", "context_window": 1000000}]'::jsonb,
   2, 'builtin', TRUE, 'medium'),
  ('claude-sonnet-5-5', 'Sonnet 5.5',
   '[{"provider": "vertex", "context_window": 1000000},
     {"provider": "anthropic", "context_window": 1000000},
     {"provider": "claude-gateway", "context_window": 1000000}]'::jsonb,
   4, 'builtin', TRUE, 'high')
ON CONFLICT (id) DO NOTHING;

-- A bare row's window used to come from its twin. Without a declared window a
-- bare `claude-` id reads as 200k, which the Fable 5.1 seed left on both its
-- routes. A route that declares its own window keeps it.
UPDATE models
   SET routes = (
         SELECT jsonb_agg(
                  CASE WHEN route ? 'context_window' THEN route
                       ELSE route || '{"context_window": 1000000}'::jsonb
                  END
                  ORDER BY n)
           FROM jsonb_array_elements(routes) WITH ORDINALITY AS r (route, n)
       ),
       updated_at = NOW()
 WHERE source = 'builtin'
   AND id IN ('claude-fable-5-1', 'claude-opus-5-5', 'claude-sonnet-5-5')
   AND EXISTS (SELECT 1 FROM jsonb_array_elements(routes) AS r (route)
                WHERE NOT route ? 'context_window');

-- Each successor always thinks, so none takes `none`.
SELECT retire_model('claude-fable-5-1[1m]', 'claude-fable-5-1', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-opus-5-5[1m]', 'claude-opus-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-sonnet-5-5[1m]', 'claude-sonnet-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);

-- 20261018074151 retired these to the twins above. Re-point them, so no
-- retired model retires to another retired model.
SELECT retire_model('claude-opus-5[1m]', 'claude-opus-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-sonnet-5[1m]', 'claude-sonnet-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-fable-5[1m]', 'claude-fable-5-1', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-opus-4-8[1m]', 'claude-opus-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-opus-4-7[1m]', 'claude-opus-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-opus-4-6[1m]', 'claude-opus-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);
SELECT retire_model('claude-sonnet-4-6[1m]', 'claude-sonnet-5-5', ARRAY['low', 'medium', 'high', 'xhigh', 'max']);

-- The provider answers a twin call with the bare id, which is what
-- `served_model` records on rows written since that field existed. Older rows
-- get the same reply id, so Token Cost reads one model. `model` keeps what
-- Lucidos sent, and a reply id already recorded stays (ADR 0248 amendment).
UPDATE events
   SET payload = payload || jsonb_build_object(
         'served_model', replace(payload ->> 'model', '[1m]', ''))
 WHERE event_type = 'ContextCaptured'
   AND payload ->> 'model' IN ('claude-fable-5-1[1m]', 'claude-opus-5-5[1m]', 'claude-sonnet-5-5[1m]')
   AND NOT payload ? 'served_model';
