-- `waiting_for_event` is no longer a thread status.
--
-- A thread holding an event wait used to have its turn parked on that wait, so
-- it carried a status of its own and counted as blocking. Since 2026-08-06 a
-- subscription does not hold a turn: `await_event` returns immediately, the
-- turn terminates normally, and the thread is plain idle while it watches. See
-- `docs/plans/2026-08-06-every-event-wait-is-detached.md`.
--
-- Two things to settle for rows written by the old engine.

-- 1. The status itself. `idle` is what those threads now mean, and leaving the
--    string in place would make every read fall through `ThreadStatus::parse`'s
--    defensive catch-all rather than being deliberate about it.
UPDATE thread_summaries
SET status = 'idle'
WHERE status = 'waiting_for_event';

-- 2. The rolled-up counters. `is_blocking` counted the old status, so an
--    ancestor of a subscribed thread carries a count that includes it, which
--    keeps its Archive button hidden for as long as the stale count survives.
--    Recompute both columns from ground truth with the CURRENT predicate.
--
--    The FILTER clauses below mirror `thread_lifecycle::is_blocking` /
--    `is_attention_needing`, inlined because a CTE cannot be shared across
--    migrations (same note as
--    `20260518132821_blocking_count_running_overrides_archived.sql`). Keep them
--    in step with those functions if the predicate changes again.
WITH RECURSIVE descendants AS (
    SELECT t.thread_id AS root,
           c.thread_id,
           c.status,
           c.archive_state,
           c.is_coding_agent,
           c.coding_agent_proposed,
           c.coding_agent_is_external_repo
    FROM thread_summaries t
    JOIN thread_summaries c ON c.parent_thread_id = t.thread_id
    UNION ALL
    SELECT d.root,
           c.thread_id,
           c.status,
           c.archive_state,
           c.is_coding_agent,
           c.coding_agent_proposed,
           c.coding_agent_is_external_repo
    FROM descendants d
    JOIN thread_summaries c ON c.parent_thread_id = d.thread_id
),
counts AS (
    SELECT root,
           COUNT(*) FILTER (
               WHERE status IN ('running', 'waiting_for_user_answer')
                  OR ( archive_state IS DISTINCT FROM 'archived'
                       AND coding_agent_proposed
                       AND is_coding_agent
                       AND NOT coding_agent_is_external_repo )
           ) AS blocking_cnt,
           COUNT(*) FILTER (
               WHERE status = 'waiting_for_user_answer'
                  OR ( archive_state IS DISTINCT FROM 'archived'
                       AND coding_agent_proposed
                       AND is_coding_agent
                       AND NOT coding_agent_is_external_repo )
           ) AS attention_cnt
    FROM descendants
    GROUP BY root
)
UPDATE thread_summaries ts
SET blocking_descendant_count = counts.blocking_cnt,
    attention_descendant_count = counts.attention_cnt
FROM counts
WHERE ts.thread_id = counts.root
  AND ( ts.blocking_descendant_count IS DISTINCT FROM counts.blocking_cnt
        OR ts.attention_descendant_count IS DISTINCT FROM counts.attention_cnt );

-- A thread with no children at all is not in the join above, so zero any stale
-- count one still carries.
UPDATE thread_summaries ts
SET blocking_descendant_count = 0,
    attention_descendant_count = 0
WHERE (ts.blocking_descendant_count <> 0 OR ts.attention_descendant_count <> 0)
  AND NOT EXISTS (
      SELECT 1 FROM thread_summaries c WHERE c.parent_thread_id = ts.thread_id
  );
