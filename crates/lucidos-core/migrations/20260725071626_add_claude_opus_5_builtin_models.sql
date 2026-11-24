-- Seed Claude Opus 5 (released 2026-07-24) into the chat model registry.
-- Served over the Vertex provider; the Google Cloud model id is `claude-opus-5`
-- (same as the Claude API id). Mirrors the Opus 4.8 rows exactly: the `@default`
-- version alias is placed verbatim in the Vertex streamRawPredict URL, and the
-- `[1m]` suffix selects the 1M-context beta (a Lucidos convention stripped before
-- the request — see anthropic_wire::parse_context_suffix).
--
-- sort_order 5/6 places Opus 5 between Fable 5 (0/1, the tier above Opus) and
-- Opus 4.8 (10/11), so the newest Opus surfaces first while Opus 4.8 stays
-- enabled below it. Builtin = disable-only (never deletable), enabled by default.
-- ON CONFLICT DO NOTHING so re-running against a populated table is a no-op.
INSERT INTO models (id, label, provider, sort_order, source) VALUES
  ('claude-opus-5@default',     'Opus 5',      'vertex', 5, 'builtin'),
  ('claude-opus-5@default[1m]', 'Opus 5 (1M)', 'vertex', 6, 'builtin')
ON CONFLICT (id) DO NOTHING;
