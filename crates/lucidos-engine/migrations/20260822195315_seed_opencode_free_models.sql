-- Seed the keyless OpenCode Free models into the chat model registry.
-- Served anonymously over OpenCode's Zen relay at opencode.ai/zen/v1, which
-- speaks OpenAI Chat Completions. Builtin = disable-only (never deletable),
-- enabled by default like the other builtins. ON CONFLICT DO NOTHING so
-- re-running against a populated table is a no-op.
--
-- Seeding a row does NOT switch the tier on. The provider is built only when
-- the `opencode_free_enabled` preference (Settings → Models → Providers) is
-- true, so until then these rows are filtered out of the picker exactly like
-- any other unconfigured provider's models.
--
-- Every window is DECLARED: the id-shape fallback has no rule for these ids
-- and would budget a 1M-context model at 200k.
--
-- `big-pickle` is deliberately absent. The relay serves it only to the OpenCode
-- CLI's own User-Agent and answers everyone else with a 429, so seeding it
-- would put a model in the picker that can never answer us.
INSERT INTO models (id, label, provider, sort_order, source, context_window) VALUES
  ('laguna-s-2.1-free',               'Laguna S 2.1 (free)',           'opencode-free', 60, 'builtin',  256000),
  ('nemotron-3.5-lightning-free',     'Nemotron 3.5 Lightning (free)', 'opencode-free', 61, 'builtin',  262144),
  ('x-preview-f-free',                'Ox Alpha (free)',               'opencode-free', 62, 'builtin', 1000000),
  ('nemotron-3-ultra-free',           'Nemotron 3 Ultra (free)',       'opencode-free', 63, 'builtin', 1000000),
  ('muse-spark-1.2-contributor-free', 'Muse Spark 1.2 (free)',         'opencode-free', 64, 'builtin', 1048576),
  ('hy3-free',                        'Hy3 (free)',                    'opencode-free', 65, 'builtin',  190000)
ON CONFLICT (id) DO NOTHING;
