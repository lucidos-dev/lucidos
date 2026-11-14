-- Declare the real context window on the remaining builtins we can verify.
--
-- The previous migration (20260725200708) added the column and seeded the four
-- OpenRouter / Gemini rows. This one finishes the job for OpenAI, and declares
-- the Claude `[1m]` rows explicitly so the picker is self-describing.
--
-- WHY THE BARE CLAUDE ROWS ARE DELIBERATELY LEFT ALONE
--
-- Current Claude models do advertise a 1M context window, so it is tempting to
-- declare 1000000 on every `claude-*` row. That would be wrong *for Lucidos*,
-- because the window that matters here is the window of the request the engine
-- actually makes -- not the model's theoretical maximum.
--
-- Lucidos gates 1M mode on its own `[1m]` id suffix: `parse_context_suffix`
-- returns `is_1m`, and only when that is true does `build_claude_request` attach
-- the `context-1m-2025-08-07` beta (as a body field on Vertex, an HTTP header on
-- the direct Anthropic path). A bare id sends no 1M beta at all. Declaring 1M on
-- a bare row would let the context packer build a prompt larger than the API
-- mode the request actually selected, and the provider would reject it -- the
-- dangerous direction. `claude-` -> 200k in the prefix map is therefore correct
-- for bare ids, and those rows stay NULL so they keep tracking it.
--
-- `gpt-5` -> 400k has no such excuse: the OpenAI path has no context opt-in, so
-- the model's full window applies to every request. GPT-5.5, GPT-5.5 Pro, and
-- the GPT-5.6 family are all 1,050,000, and the map understates them.

-- Claude `[1m]` rows: these DO request 1M mode, so the declared window matches
-- the request. Same value the prefix map already infers from the suffix --
-- recorded on the row so Settings shows a real number instead of "inferred".
UPDATE models SET context_window = 1000000, updated_at = NOW()
WHERE context_window IS NULL
  AND id IN (
    'claude-fable-5[1m]',
    'claude-opus-5@default[1m]',
    'claude-opus-4-8@default[1m]',
    'claude-opus-4-7[1m]',
    'claude-opus-4-6[1m]',
    'claude-sonnet-4-6[1m]'
  );

-- OpenAI: the GPT-5.5 and GPT-5.6 families are 1,050,000, not the 400k guess.
-- (Some third-party listings quote 922K for these. That is the max INPUT you can
-- send while still reserving the full 128K output, not the window. This column
-- holds the vendor's stated context window on every row -- output headroom is
-- carved out separately and uniformly by `RESPONSE_TOKEN_RESERVE`.)
UPDATE models SET context_window = 1050000, updated_at = NOW()
WHERE context_window IS NULL
  AND id IN (
    'gpt-5.5-pro',
    'gpt-5.5',
    'gpt-5.6-sol',
    'gpt-5.6-terra',
    'gpt-5.6-luna'
  );

-- Still undeclared on purpose:
--   * every bare `claude-*` row -- see the note above; the prefix map's 200k
--     matches the non-beta request Lucidos actually makes.
--   * claude-opus-4-5@20251101, gpt-5.4, gpt-5.3-codex, gpt-5.3-codex-spark,
--     gpt-5.2-codex -- windows unverified, and an over-declared window is worse
--     than the fallback (rejected request vs. trimming early).
--   * user rows (e.g. moonshotai/kimi-k3) -- a migration cannot seed those; the
--     user declares them in Settings -> Models.
