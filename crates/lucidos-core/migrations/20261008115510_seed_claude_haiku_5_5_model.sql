-- Seed Claude Haiku 5.5 into the chat model registry.
--
-- Haiku 5.5 is published on Vertex and on the direct Anthropic API under the
-- same bare id, so neither route spells an `id`. OpenRouter lists it as
-- `anthropic/claude-haiku-5.5`. The routes follow Haiku 4.5's row
-- (20261006172127): first party first, so a workspace with Vertex or an
-- Anthropic key never leaves for OpenRouter.
--
-- Every route declares 1000000: the model has one window, 1M, on each backend.
-- It reads images, so the vision flag is set.
-- See docs/plans/2026-10-08-haiku-5-5-and-a-tree-memory-eval.md.
--
-- sort_order 9 is the free integer between Sonnet 5 (1M) (8) and Opus 4.8
-- (10), which keeps the current generation together at the top of the picker.
--
-- Builtin = disable-only, enabled by default. ON CONFLICT DO NOTHING leaves a
-- user-created row under the id untouched, and makes a re-run a no-op.
INSERT INTO models (id, label, routes, sort_order, source, vision) VALUES
  ('claude-haiku-5-5', 'Haiku 5.5',
   '[{"provider": "vertex", "context_window": 1000000},
     {"provider": "anthropic", "context_window": 1000000},
     {"provider": "openrouter", "id": "anthropic/claude-haiku-5.5", "context_window": 1000000}]'::jsonb,
   9, 'builtin', TRUE)
ON CONFLICT (id) DO NOTHING;
