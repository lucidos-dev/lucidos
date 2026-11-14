-- The drawer's Archive section now sorts and paginates by `created_at` (matching
-- the date each row displays), instead of `last_user_action`. Add the matching
-- sort indexes — plain (for `get_older_threads`' `created_at < $1 ORDER BY
-- created_at DESC` scroll-page) and per-source (for `get_recent_threads`'
-- per-source `ROW_NUMBER() ... ORDER BY created_at DESC` initial window) —
-- mirroring the existing `last_activity` / `last_user_action` index pairs so the
-- archive queries don't fall back to seq scans as `thread_summaries` grows.
CREATE INDEX IF NOT EXISTS idx_thread_summaries_created_at
    ON thread_summaries (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_thread_summaries_source_created_at
    ON thread_summaries (source, created_at DESC);
