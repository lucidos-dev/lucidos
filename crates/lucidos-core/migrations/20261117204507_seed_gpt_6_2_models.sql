-- Add the GPT-6.2 models without replacing existing user rows.
-- OpenAI documents no API default effort for either model.
INSERT INTO models (id, label, routes, sort_order, source, vision, default_effort) VALUES
  ('gpt-6.2-astra', 'GPT-6.2 Astra',
   '[{"provider": "chatgpt-plan", "context_window": 1050000},
     {"provider": "openai", "context_window": 1050000}]'::jsonb,
   34, 'builtin', TRUE, NULL),
  ('gpt-6.2-sol', 'GPT-6.2 Sol',
   '[{"provider": "chatgpt-plan", "context_window": 1050000},
     {"provider": "openai", "context_window": 1050000},
     {"provider": "openrouter", "id": "openai/gpt-6.2-sol", "context_window": 1050000}]'::jsonb,
   35, 'builtin', TRUE, NULL)
ON CONFLICT (id) DO NOTHING;
