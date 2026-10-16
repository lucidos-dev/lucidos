-- Seed OpenAI's GPT-5.6 family (Sol / Terra / Luna) into the chat model
-- registry. Served over the direct `openai` provider; GPT-5.6 ids start with
-- `gpt-5`, so they route through the Responses API automatically. Sol is the
-- frontier tier, Terra the balanced middle, Luna the fast/cheap tier.
--
-- sort_order 37/38/39 places the family at the top of the OpenAI cluster (just
-- above GPT-5.5 Pro at 40), so the newest models surface first. Builtin =
-- disable-only (never deletable), enabled by default like the other builtins.
-- ON CONFLICT DO NOTHING so re-running against a populated table is a no-op.
INSERT INTO models (id, label, provider, sort_order, source) VALUES
  ('gpt-5.6-sol',   'GPT-5.6 Sol',   'openai', 37, 'builtin'),
  ('gpt-5.6-terra', 'GPT-5.6 Terra', 'openai', 38, 'builtin'),
  ('gpt-5.6-luna',  'GPT-5.6 Luna',  'openai', 39, 'builtin')
ON CONFLICT (id) DO NOTHING;
