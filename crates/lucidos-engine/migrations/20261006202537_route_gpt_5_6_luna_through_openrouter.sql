-- Make GPT-5.6 Luna, the fact-extraction lead, reachable on OpenRouter.
--
-- Extraction runs on the first of GPT-5.6 Luna, Gemini 3 Flash, GPT-5.4 mini
-- and Haiku 4.5 that a configured provider serves. The other three gained
-- OpenRouter routes in 20261006172127_seed_background_default_models.sql.
-- Without this one an OpenRouter-only install extracts on Gemini 3 Flash.
--
-- The OpenAI route stays first, so a workspace with OpenAI configured never
-- leaves for OpenRouter. Window: 1,050,000 tokens, matching the OpenAI route
-- and OpenRouter's listing for `openai/gpt-5.6-luna`.
--
-- Only the builtin row gains the route, and only while it has none: a row the
-- user owns keeps the routes they chose, and a re-run is a no-op.
UPDATE models
SET routes = routes || '[{"provider": "openrouter", "id": "openai/gpt-5.6-luna", "context_window": 1050000}]'::jsonb
WHERE id = 'gpt-5.6-luna'
  AND source = 'builtin'
  AND NOT routes @> '[{"provider": "openrouter"}]'::jsonb;
