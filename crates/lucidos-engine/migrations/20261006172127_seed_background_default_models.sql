-- Make the auxiliary default's models reachable through the router.
--
-- An auxiliary model call (titles, memory, summaries, the command judge) runs
-- on the first of Gemini 3 Flash, GPT-5.4 mini and Haiku 4.5 that a configured
-- provider serves (docs/plans/2026-10-06-auxiliary-calls-through-the-router.md).
-- Without a row, the prefix heuristic sends `claude-haiku-4-5` to Vertex only
-- and `gpt-5.4-mini` to OpenAI only, so an Anthropic-only or OpenRouter-only
-- install could reach neither.
--
-- The first-party route stays first in each row, so a workspace with it
-- configured never leaves for OpenRouter. Windows: Haiku 4.5 200,000 and
-- GPT-5.4 mini 400,000 tokens, from OpenRouter's listing.
--
-- sort_order joins each family's neighbours: 18 after the Claude 4.6 rows, 43
-- after GPT-5.4. Builtin = disable-only, enabled by default. ON CONFLICT DO
-- NOTHING leaves a user-created row under either id untouched.
INSERT INTO models (id, label, routes, sort_order, source) VALUES
  ('claude-haiku-4-5', 'Haiku 4.5',
   '[{"provider": "vertex", "context_window": 200000},
     {"provider": "anthropic", "context_window": 200000},
     {"provider": "openrouter", "id": "anthropic/claude-haiku-4.5", "context_window": 200000}]'::jsonb,
   18, 'builtin'),
  ('gpt-5.4-mini', 'GPT-5.4 mini',
   '[{"provider": "openai", "context_window": 400000},
     {"provider": "openrouter", "id": "openai/gpt-5.4-mini", "context_window": 400000}]'::jsonb,
   43, 'builtin')
ON CONFLICT (id) DO NOTHING;

-- Gemini 3 Flash heads the default, so OpenRouter must be able to serve it.
-- Only the builtin row gains the route, and only while it has none: a row the
-- user owns keeps the routes they chose, and a re-run is a no-op.
UPDATE models
SET routes = routes || '[{"provider": "openrouter", "id": "google/gemini-3-flash-preview", "context_window": 1048576}]'::jsonb
WHERE id = 'gemini-3-flash-preview'
  AND source = 'builtin'
  AND NOT routes @> '[{"provider": "openrouter"}]'::jsonb;
