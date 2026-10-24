-- Prune the prior generation of every builtin family from the picker.
--
-- Same shape as 20260615124227, and for the same reason. Builtins are
-- disable-only: the registry never deletes them, so the user can switch one
-- back on from Settings -> Models, and `model_registry::load_from_db` keeps
-- loading disabled rows so a saved `chat_model` naming one still routes. A
-- fresh install runs the seeds (all enabled) and then this, so fresh and
-- existing installs converge on the same curated set.
--
-- Switched off here:
--   * Claude prior generation -- Opus 4.8, Opus 4.7 and Sonnet 4.6 with their
--     `[1m]` rows, superseded by Opus 5 and Sonnet 5.
--   * Older OpenAI -- GPT-5.5, GPT-5.4 and GPT-5.3 Codex, superseded by the
--     GPT-5.6 family.
--   * Older Grok -- 4.5 and 4.3, superseded by 4.6.
--
-- Kept on: Fable 5, Opus 5, Sonnet 5 (each with its `[1m]` row), GPT-5.6 Sol /
-- Terra / Luna, GPT-5.5 Pro, Grok 4.6, Grok 4.20 (its 2M window has no peer in
-- the family), every Gemini row, GLM 5.2 and the OpenCode Free tier.
--
-- `source = 'builtin'` scopes the write, so a user row added under one of these
-- ids keeps whatever state its owner gave it.
UPDATE models
SET enabled = FALSE, updated_at = NOW()
WHERE source = 'builtin'
  AND id IN (
    'claude-opus-4-8@default',
    'claude-opus-4-8@default[1m]',
    'claude-opus-4-7',
    'claude-opus-4-7[1m]',
    'claude-sonnet-4-6',
    'claude-sonnet-4-6[1m]',
    'gpt-5.5',
    'gpt-5.4',
    'gpt-5.3-codex',
    'grok-4.5',
    'grok-4.3'
  );
