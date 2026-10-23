-- Seed GPT-6.1 Sol and Gemini 3.8 Flash into the model registry.
--
-- Both lead the Tree compactor's provider-aware default
-- (docs/plans/2026-10-06-tree-compactor-provider-aware-default.md). Each needs
-- a row because each also routes through OpenRouter, under a prefixed id, and
-- the prefix heuristic alone would reach only the first-party backend. The
-- first-party route stays first, so a workspace with it configured never
-- leaves for OpenRouter.
--
-- Windows: GPT-6.1 Sol 1,050,000 tokens (OpenAI's model page), Gemini 3.8
-- Flash 1,048,576 (OpenRouter's listing, the Gemini Flash window).
--
-- sort_order joins each family's neighbours: 36 beside GPT-6 Astra, 31 beside
-- Gemini 3.5 Flash. The registry orders by (sort_order, label).
--
-- Builtin = disable-only, enabled by default. ON CONFLICT DO NOTHING leaves a
-- user-created row under either id untouched, and makes a re-run a no-op.
INSERT INTO models (id, label, routes, sort_order, source) VALUES
  ('gpt-6.1-sol', 'GPT-6.1 Sol',
   '[{"provider": "openai", "context_window": 1050000},
     {"provider": "openrouter", "id": "openai/gpt-6.1-sol", "context_window": 1050000}]'::jsonb,
   36, 'builtin'),
  ('gemini-3.8-flash', 'Gemini 3.8 Flash',
   '[{"provider": "vertex", "context_window": 1048576},
     {"provider": "openrouter", "id": "google/gemini-3.8-flash", "context_window": 1048576}]'::jsonb,
   31, 'builtin')
ON CONFLICT (id) DO NOTHING;
