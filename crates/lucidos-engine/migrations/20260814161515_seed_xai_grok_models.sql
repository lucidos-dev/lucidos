-- Seed the Grok family on the xAI provider into the chat model registry.
-- Served over xAI's OpenAI-compatible Chat Completions API at api.x.ai/v1.
-- Builtin = disable-only (never deletable), enabled by default like the other
-- builtins. Each row shows in the picker and surfaces a clear "xAI not
-- configured" error at call time when no `xai` credential (Settings → Models →
-- Providers) and no LUCIDOS_XAI_API_KEY is set. ON CONFLICT DO NOTHING so
-- re-running against a populated table is a no-op.
--
-- The ids are BARE here, which is how xAI itself addresses them. OpenRouter
-- prefixes the same models (`x-ai/grok-4.6`), and such a row is a separate
-- registry entry on the `openrouter` provider. Both coexist: the registry is
-- keyed on the full id, so seeding these cannot re-route a Grok model a user
-- already reaches through OpenRouter.
--
-- Every window is DECLARED, because the id-shape fallback has no rule for a
-- `grok-` id and would budget all four at 200k.
INSERT INTO models (id, label, provider, sort_order, source, context_window) VALUES
  ('grok-4.6',  'Grok 4.6',  'xai', 55, 'builtin',   500000),
  ('grok-4.5',  'Grok 4.5',  'xai', 56, 'builtin',   500000),
  ('grok-4.20', 'Grok 4.20', 'xai', 57, 'builtin',  2000000),
  ('grok-4.3',  'Grok 4.3',  'xai', 58, 'builtin',  1000000)
ON CONFLICT (id) DO NOTHING;
