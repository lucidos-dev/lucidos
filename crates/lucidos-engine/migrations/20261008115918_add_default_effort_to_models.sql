-- A model's *default effort*: the reasoning effort it runs at when nothing
-- stored names one. See docs/plans/2026-10-08-each-model-runs-at-its-default-effort.md.
--
-- Per model, not per route: the provider documents it per model. A route that
-- cannot run it snaps it, as for any other tier.
--
-- NULL means no documented default, so the engine sends no effort and the
-- server applies its own. A user row starts there.

ALTER TABLE models ADD COLUMN IF NOT EXISTS default_effort TEXT
    CHECK (default_effort IN ('none', 'low', 'medium', 'high', 'xhigh', 'max'));

-- Seeds: the provider's documented default, else its documented
-- recommendation. A model that does not think by default takes `none`.
-- `source = 'builtin'` leaves a user row alone, and `default_effort IS NULL`
-- makes a re-run a no-op.
--
-- Sources:
--   https://platform.claude.com/docs/en/build-with-claude/effort
--   https://platform.claude.com/docs/en/build-with-claude/thinking-troubleshooting
--   https://developers.openai.com/api/docs/models/<model id>
--   https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide
--   https://ai.google.dev/gemini-api/docs/thinking
--   https://docs.x.ai/developers/model-capabilities/text/reasoning
--   https://docs.x.ai/developers/models/grok-4.3
--
-- Left NULL as undocumented: gpt-6-astra, gpt-5.2-codex, gpt-5.3-codex-spark,
-- grok-4.20, z-ai/glm-5.2 and the OpenCode free rows.
UPDATE models SET default_effort = seed.effort, updated_at = NOW()
FROM (VALUES
    ('claude-fable-5', 'high'),
    ('claude-fable-5[1m]', 'high'),
    ('claude-fable-5-1', 'high'),
    ('claude-fable-5-1[1m]', 'high'),
    ('claude-opus-5', 'high'),
    ('claude-opus-5[1m]', 'high'),
    ('claude-opus-5-5', 'medium'),
    ('claude-opus-5-5[1m]', 'medium'),
    ('claude-sonnet-5', 'high'),
    ('claude-sonnet-5[1m]', 'high'),
    ('claude-sonnet-5-5', 'high'),
    ('claude-sonnet-5-5[1m]', 'high'),
    ('claude-opus-4-8', 'none'),
    ('claude-opus-4-8[1m]', 'none'),
    ('claude-opus-4-7', 'none'),
    ('claude-opus-4-7[1m]', 'none'),
    ('claude-opus-4-6', 'none'),
    ('claude-opus-4-6[1m]', 'none'),
    ('claude-opus-4-5@20251101', 'none'),
    ('claude-sonnet-4-6', 'none'),
    ('claude-sonnet-4-6[1m]', 'none'),
    ('claude-haiku-4-5', 'none'),
    ('claude-haiku-5-5', 'medium'),
    ('gpt-6.1-sol', 'medium'),
    ('gpt-5.6-sol', 'medium'),
    ('gpt-5.6-terra', 'medium'),
    ('gpt-5.6-luna', 'medium'),
    ('gpt-5.5', 'medium'),
    ('gpt-5.5-pro', 'high'),
    ('gpt-5.4', 'none'),
    ('gpt-5.4-mini', 'none'),
    ('gpt-5.3-codex', 'medium'),
    ('gemini-3.8-flash', 'medium'),
    ('gemini-3.5-flash', 'medium'),
    ('gemini-3.1-pro-preview', 'high'),
    ('gemini-3-flash-preview', 'high'),
    ('grok-4.6', 'high'),
    ('grok-4.5', 'high'),
    ('grok-4.3', 'low')
) AS seed(id, effort)
WHERE models.id = seed.id
  AND models.source = 'builtin'
  AND models.default_effort IS NULL;
