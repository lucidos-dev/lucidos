-- Materialized count of descendants (transitive) currently in a state that
-- prevents this thread's archive. Consumed by resolve_actions via a
-- `count > 0` bool. Updated by the projection in event_bus_projection.rs
-- on every event that can flip a descendant's "blocking" predicate.
--
-- A thread T is "blocking" iff:
--   T.archive_state <> 'archived'  AND
--   (T.status IN ('running','waiting_for_user_answer')
--    OR (T.coding_agent_proposed AND T.is_coding_agent))
--
-- Naming note: the design doc uses the conceptual names `has_pending_changes`
-- and `thread_type` for these fields; the actual `thread_summaries` columns
-- are `coding_agent_proposed` (the boolean set by ChangeProposed / cleared by
-- ChangeApplied / ChangeDiscarded) and `is_coding_agent` (the boolean set on
-- SessionStarted that distinguishes CC threads from Chat threads).

ALTER TABLE thread_summaries
    ADD COLUMN blocking_descendant_count INTEGER NOT NULL DEFAULT 0;

-- One-shot backfill via recursive CTE.
WITH RECURSIVE descendants AS (
    SELECT t.thread_id AS root_id,
           c.thread_id, c.status, c.archive_state,
           c.coding_agent_proposed, c.is_coding_agent
    FROM thread_summaries t
    JOIN thread_summaries c ON c.parent_thread_id = t.thread_id
    UNION ALL
    SELECT d.root_id,
           c.thread_id, c.status, c.archive_state,
           c.coding_agent_proposed, c.is_coding_agent
    FROM descendants d
    JOIN thread_summaries c ON c.parent_thread_id = d.thread_id
)
UPDATE thread_summaries u
SET blocking_descendant_count = sub.cnt
FROM (
    SELECT root_id,
           COUNT(*) FILTER (
               WHERE archive_state <> 'archived'
                 AND (status IN ('running','waiting_for_user_answer')
                      OR (coding_agent_proposed
                          AND is_coding_agent))
           ) AS cnt
    FROM descendants
    GROUP BY root_id
) sub
WHERE u.thread_id = sub.root_id;
