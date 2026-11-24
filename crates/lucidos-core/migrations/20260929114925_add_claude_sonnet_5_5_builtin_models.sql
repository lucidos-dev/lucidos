-- Seed Claude Sonnet 5.5 into the chat model registry.
--
-- Sonnet 5.5 is published on Vertex and on the direct Anthropic API under the
-- same bare id, so neither route spells an `id`. Vertex stays first, like the
-- other current-generation Claude rows (20260922215055).
--
-- Both rows declare 1000000 on each route. The model has no 200k variant: its
-- bare request already runs 1M, like the families 20260923075003 declares. The
-- `[1m]` twin adds only the 1M beta, which the model ignores. It is kept so
-- Sonnet 5.5 offers the same pair as Opus 5.5 and Fable.
--
-- sort_order 4 is the one free integer between Opus 5.5 (2/3) and Opus 5
-- (5/6). Both rows share it, and the registry orders by (sort_order, label), so
-- "Sonnet 5.5" lists before "Sonnet 5.5 (1M)". No renumbering: sort_order is
-- user-editable through the models API.
--
-- Builtin = disable-only, enabled by default. ON CONFLICT DO NOTHING leaves a
-- user-created row under either id untouched, and makes a re-run a no-op.
INSERT INTO models (id, label, routes, sort_order, source) VALUES
  ('claude-sonnet-5-5', 'Sonnet 5.5',
   '[{"provider": "vertex", "context_window": 1000000},
     {"provider": "anthropic", "context_window": 1000000}]'::jsonb,
   4, 'builtin'),
  ('claude-sonnet-5-5[1m]', 'Sonnet 5.5 (1M)',
   '[{"provider": "vertex", "context_window": 1000000},
     {"provider": "anthropic", "context_window": 1000000}]'::jsonb,
   4, 'builtin')
ON CONFLICT (id) DO NOTHING;
