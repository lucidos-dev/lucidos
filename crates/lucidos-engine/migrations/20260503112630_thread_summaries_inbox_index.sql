-- Partial index supporting `get_recent_threads`'s unbounded inbox scan.
-- Most rows are 'archived' so a partial index on inbox stays small while
-- letting the planner skip the table-scan.
CREATE INDEX IF NOT EXISTS idx_thread_summaries_archive_state_inbox
    ON thread_summaries (last_activity DESC)
    WHERE archive_state = 'inbox';
