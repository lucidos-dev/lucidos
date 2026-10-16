-- The Thread Queue reads user occupancy at every admission (ADR 0443): the
-- user-initiated threads that are running. The predicate must match
-- `ThreadQueue::read_user_occupancy`, or the planner cannot use this index.
CREATE INDEX IF NOT EXISTS idx_thread_summaries_running_user
    ON thread_summaries (thread_id)
    WHERE status = 'running' AND initiator = 'user';
