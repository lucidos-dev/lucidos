-- Prune superseded builtin chat models from the registry (June 2026 lineup).
--
-- Builtins are disable-only by design (the registry never deletes them, so the
-- user can re-enable from Settings → Models). Setting enabled = FALSE removes
-- them from the picker while keeping the row. Fresh installs run the seed
-- (20260610152555, all enabled) and then this disable, so existing and fresh
-- installs converge on the same curated set.
--
-- Pruned: Opus 4.6 (+1M) and Opus 4.5 (prior-gen Opus, superseded by 4.8/4.7),
-- and GPT-5.2 Codex + Codex Spark (superseded OpenAI coding models). Kept:
-- Fable 5, Opus 4.8/4.7, Sonnet 4.6, current Gemini, GPT-5.5/5.5 Pro/5.4 and
-- GPT-5.3 Codex.
UPDATE models
SET enabled = FALSE, updated_at = NOW()
WHERE source = 'builtin'
  AND id IN (
    'claude-opus-4-6',
    'claude-opus-4-6[1m]',
    'claude-opus-4-5@20251101',
    'gpt-5.2-codex',
    'gpt-5.3-codex-spark'
  );
