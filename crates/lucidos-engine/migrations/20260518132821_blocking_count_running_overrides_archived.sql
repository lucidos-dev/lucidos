-- Two coupled fixes for the cascading-archive race.
--
-- 1. Change the `archive_state` column default from 'archived' to 'inbox'.
--    The old default meant freshly-spawned rows (CC sub-thread on MessageReceived,
--    chat sub-thread on MessageReceived) sat at 'archived' until their first
--    section-transitioning event fired — `CodingAgentIdled`, `ChangeProposed`,
--    `UserQuestionAsked`, etc. During that window the thread was actively
--    running but the row's `archive_state` was indistinguishable from
--    "user explicitly archived this thread", so the `is_blocking` predicate's
--    archived short-circuit returned false and the parent's
--    `blocking_descendant_count` undercounted active descendants. The user
--    could see (and click) the parent's Archive button while a CC sub-thread
--    was actively streaming tool calls. 'inbox' is the right default:
--    every section-transitioning event that today reaches 'inbox' becomes a
--    no-op on a fresh row, and the legacy short-circuit no longer conflates
--    "user-archived" with "newly created".
--
-- 2. Re-backfill `blocking_descendant_count` to match the updated `is_blocking`
--    predicate (Rust: `crates/lucidos-engine/src/engine/thread_lifecycle.rs`).
--    The new predicate treats Running / WaitingForUserAnswer as always
--    blocking, regardless of `archive_state`. Without this re-backfill, any
--    parent whose count was undercounted under the old predicate (because a
--    descendant was actively running at the legacy default 'archived') would
--    keep a stale value forever — the projection only updates on flips, not
--    on predicate changes. Mirrors `rebuild_blocking_descendant_count` in
--    `event_bus_projection.rs`.

ALTER TABLE thread_summaries ALTER COLUMN archive_state SET DEFAULT 'inbox';

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
               WHERE status IN ('running','waiting_for_user_answer')
                  OR (archive_state <> 'archived'
                      AND coding_agent_proposed AND is_coding_agent
                      AND NOT coding_agent_is_external_repo)
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
