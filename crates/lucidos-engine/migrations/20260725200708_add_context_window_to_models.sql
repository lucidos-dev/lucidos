-- Per-model context window for the chat-model registry.
--
-- Before this, `engine::context::context_window_from_prefix` was the only
-- source: a hardcoded prefix map ([1m] -> 1M, claude- -> 200k, gpt-5 -> 400k,
-- else 200k). Every other model — every OpenRouter / Vertex-Gemini / local row,
-- builtin or user-added — silently took the 200k fallback, so the trim budget
-- evicted context at a fraction of the real window (kimi-k3: 200k assumed vs
-- 1,048,576 actual).
--
-- NULL means "not declared — fall back to the prefix map". That keeps every
-- existing row working untouched and keeps the prefix map the single fallback
-- for ids nobody has declared.

ALTER TABLE models ADD COLUMN IF NOT EXISTS context_window INTEGER;

-- Seed ONLY the builtins the prefix map gets wrong. Claude / GPT-5 rows are
-- deliberately left NULL: the prefix map already resolves those correctly, and
-- duplicating it here would create two sources of truth that can drift.
-- `moonshotai/kimi-k3` is a source='user' row, so it cannot be seeded from a
-- migration — the user sets it in Settings -> Models.
UPDATE models SET context_window = 1048576, updated_at = NOW()
WHERE context_window IS NULL
  AND id IN (
    'z-ai/glm-5.2',            -- OpenRouter: 1,048,576
    'gemini-3.1-pro-preview',  -- Vertex: 1M
    'gemini-3.5-flash',        -- Vertex: 1M
    'gemini-3-flash-preview'   -- Vertex: 1M
  );
