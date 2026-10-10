-- Re-backfill `blocking_descendant_count` after the `is_blocking` predicate
-- was extended to exempt external-repo CC threads with pending changes from
-- blocking their ancestors. The frontend (WaitingBanner.tsx) surfaces Archive
-- (not Apply) for external-repo CC, and the cascade handler clears the
-- pending change via ChangeApplied before archiving — so external-repo CC
-- must NOT contribute to a parent's blocking count.
--
-- Without this re-backfill, any existing parent whose count was incremented
-- by an external-repo CC child with `coding_agent_proposed=true` would keep
-- a stale +1 forever (the projection only updates on flips, not on predicate
-- changes), gating its Archive action behind a descendant that should not be
-- blocking. Matches `rebuild_blocking_descendant_count` in
-- `event_bus_projection.rs`.

WITH RECURSIVE descendants AS (
    SELECT t.thread_id AS root_id,
           c.thread_id, c.status, c.archive_state,
           c.coding_agent_proposed, c.is_coding_agent,
           c.coding_agent_is_external_repo
    FROM thread_summaries t
    JOIN thread_summaries c ON c.parent_thread_id = t.thread_id
    UNION ALL
    SELECT d.root_id,
           c.thread_id, c.status, c.archive_state,
           c.coding_agent_proposed, c.is_coding_agent,
           c.coding_agent_is_external_repo
    FROM descendants d
    JOIN thread_summaries c ON c.parent_thread_id = d.thread_id
)
UPDATE thread_summaries u
SET blocking_descendant_count = COALESCE(sub.cnt, 0)
FROM (
    SELECT root_id,
           COUNT(*) FILTER (
               WHERE archive_state <> 'archived'
                 AND (status IN ('running','waiting_for_user_answer')
                      OR (coding_agent_proposed AND is_coding_agent
                          AND NOT coding_agent_is_external_repo))
           ) AS cnt
    FROM descendants
    GROUP BY root_id
) sub
WHERE u.thread_id = sub.root_id;

-- Reset leaves whose count is stale and now zero (no UPDATE…FROM above
-- touches a row with no descendants — but a leaf with stale data needs
-- to be explicitly zeroed). Mirrors the second statement in
-- `rebuild_blocking_descendant_count`.
UPDATE thread_summaries u
SET blocking_descendant_count = 0
WHERE NOT EXISTS (
    SELECT 1 FROM thread_summaries c WHERE c.parent_thread_id = u.thread_id
) AND u.blocking_descendant_count <> 0;
