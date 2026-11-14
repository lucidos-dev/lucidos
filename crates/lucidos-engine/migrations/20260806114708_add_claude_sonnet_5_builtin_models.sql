-- Seed Claude Sonnet 5 into the chat model registry.
--
-- Served over the Vertex provider under the bare first-party id
-- `claude-sonnet-5`, exactly like the Sonnet 4.6 rows: Vertex takes the bare id
-- for current-generation models, and only dated snapshots (plus the Opus rows'
-- `@default` version alias) need an `@`. The `[1m]` suffix is the Lucidos
-- convention selecting the 1M-context beta, stripped before the request by
-- anthropic_wire::parse_context_suffix.
--
-- sort_order 7/8 places Sonnet 5 between Opus 5 (5/6) and Opus 4.8 (10/11), so
-- the current generation (Fable 5, Opus 5, Sonnet 5) groups at the top of the
-- picker while Sonnet 4.6 (16/17) stays enabled below it. Free integers rather
-- than renumbering the neighbours: sort_order is user-editable through the
-- models API, so an UPDATE here would clobber a user's own ordering.
--
-- Builtin = disable-only (never deletable), enabled by default. ON CONFLICT DO
-- NOTHING so re-running against a populated table is a no-op.
INSERT INTO models (id, label, provider, sort_order, source) VALUES
  ('claude-sonnet-5',     'Sonnet 5',      'vertex', 7, 'builtin'),
  ('claude-sonnet-5[1m]', 'Sonnet 5 (1M)', 'vertex', 8, 'builtin')
ON CONFLICT (id) DO NOTHING;

-- Declare the window on the `[1m]` row only, matching the contract set by
-- 20260725211150: a declared window describes the window of the request Lucidos
-- actually makes, not the model's theoretical maximum. The `[1m]` id does
-- request 1M mode, so 1000000 is honest.
--
-- The bare row stays NULL on purpose so it keeps tracking the prefix map's
-- 200k. A bare id sends no `context-1m-2025-08-07` beta, and declaring 1M there
-- would let the context packer build a prompt larger than the API mode the
-- request selected, which the provider then rejects outright.
-- `source = 'builtin'` keeps the ID-conflict case a true no-op. The INSERT
-- above already declines to overwrite a row the user created under this id, so
-- without this condition the UPDATE would still reach in and set a window (and
-- bump updated_at) on somebody's own row. That row may point at a provider
-- where the `[1m]` suffix means nothing, which is the over-declaring direction
-- the paragraph above calls the dangerous one.
UPDATE models SET context_window = 1000000, updated_at = NOW()
WHERE context_window IS NULL
  AND source = 'builtin'
  AND id = 'claude-sonnet-5[1m]';
