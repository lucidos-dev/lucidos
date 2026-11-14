-- Partial index for the per-repo filter in `get_older_threads` and the
-- correlated `cc_repo_name` subquery in THREAD_COLS. Without it, both
-- queries fall back to seq scans as `thread_summaries` grows.
CREATE INDEX IF NOT EXISTS idx_thread_summaries_cc_repo_id
    ON thread_summaries (cc_repo_id)
    WHERE cc_repo_id IS NOT NULL;
