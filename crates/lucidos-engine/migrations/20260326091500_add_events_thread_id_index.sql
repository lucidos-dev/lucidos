-- Add index on thread_id for fast event loading by thread.
-- Without this, every thread event query does a full sequential scan of the
-- entire events table (306K+ rows, 882MB+). The composite index covers the
-- exact query pattern: WHERE thread_id = $1 ORDER BY created ASC, sequence ASC.
CREATE INDEX IF NOT EXISTS idx_events_thread_id_created_seq
    ON events (thread_id, created, sequence);

-- Also index aggregate_id which is used for thread grouping queries.
CREATE INDEX IF NOT EXISTS idx_events_aggregate_id
    ON events (aggregate_id);
