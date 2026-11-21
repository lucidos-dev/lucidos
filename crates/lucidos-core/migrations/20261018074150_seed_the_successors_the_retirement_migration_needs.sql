-- Seed every successor that 20261018074151 retires a model to.
--
-- That migration's retire_model raises when a successor has no row, and a
-- raise stops the engine at boot. A workspace can lack a row an older seed
-- wrote, so this one restores them first.
--
-- WHY THIS VERSION IS ONE SECOND EARLIER, AND HAND-PICKED
--
-- It must run before the retirement on every database that has not applied
-- it yet. sqlx applies each unapplied migration in version order, whatever is
-- already applied. A database that ran the retirement already applies this
-- afterwards, as a no-op. Editing the retirement instead would change its
-- checksum, and sqlx refuses to boot on an edited applied migration.
--
-- Each row is the state the earlier seeds and route migrations leave behind,
-- gateway routes included. ON CONFLICT DO NOTHING leaves an existing row,
-- builtin or user, untouched.
INSERT INTO models (id, label, routes, sort_order, source, vision, default_effort) VALUES
  ('claude-fable-5-1', 'Fable 5.1',
   '[{"provider": "anthropic", "context_window": 1000000},
     {"provider": "claude-gateway", "context_window": 1000000}]'::jsonb,
   -2, 'builtin', TRUE, 'high'),
  ('claude-fable-5-1[1m]', 'Fable 5.1 (1M)',
   '[{"provider": "anthropic", "context_window": 1000000},
     {"provider": "claude-gateway", "context_window": 1000000}]'::jsonb,
   -1, 'builtin', TRUE, 'high'),
  ('claude-opus-5-5', 'Opus 5.5',
   '[{"provider": "vertex", "context_window": 1000000},
     {"provider": "anthropic", "context_window": 1000000},
     {"provider": "claude-gateway", "context_window": 1000000}]'::jsonb,
   2, 'builtin', TRUE, 'medium'),
  ('claude-opus-5-5[1m]', 'Opus 5.5 (1M)',
   '[{"provider": "vertex", "context_window": 1000000},
     {"provider": "anthropic"},
     {"provider": "claude-gateway"}]'::jsonb,
   3, 'builtin', TRUE, 'medium'),
  ('claude-sonnet-5-5', 'Sonnet 5.5',
   '[{"provider": "vertex", "context_window": 1000000},
     {"provider": "anthropic", "context_window": 1000000},
     {"provider": "claude-gateway", "context_window": 1000000}]'::jsonb,
   4, 'builtin', TRUE, 'high'),
  ('claude-sonnet-5-5[1m]', 'Sonnet 5.5 (1M)',
   '[{"provider": "vertex", "context_window": 1000000},
     {"provider": "anthropic", "context_window": 1000000},
     {"provider": "claude-gateway", "context_window": 1000000}]'::jsonb,
   4, 'builtin', TRUE, 'high'),
  ('claude-haiku-5-5', 'Haiku 5.5',
   '[{"provider": "vertex", "context_window": 1000000},
     {"provider": "anthropic", "context_window": 1000000},
     {"provider": "claude-gateway", "context_window": 1000000},
     {"provider": "openrouter", "id": "anthropic/claude-haiku-5.5", "context_window": 1000000}]'::jsonb,
   9, 'builtin', TRUE, 'medium'),
  ('gemini-3.8-flash', 'Gemini 3.8 Flash',
   '[{"provider": "vertex", "context_window": 1048576},
     {"provider": "openrouter", "id": "google/gemini-3.8-flash", "context_window": 1048576}]'::jsonb,
   31, 'builtin', TRUE, 'medium'),
  ('gpt-6.1-sol', 'GPT-6.1 Sol',
   '[{"provider": "openai", "context_window": 1050000},
     {"provider": "openrouter", "id": "openai/gpt-6.1-sol", "context_window": 1050000}]'::jsonb,
   36, 'builtin', TRUE, 'medium'),
  ('gpt-6-astra', 'GPT-6 Astra',
   '[{"provider": "openai", "context_window": 1050000}]'::jsonb,
   36, 'builtin', TRUE, NULL)
ON CONFLICT (id) DO NOTHING;
