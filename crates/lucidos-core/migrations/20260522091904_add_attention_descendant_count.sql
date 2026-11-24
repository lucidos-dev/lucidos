-- Add `attention_descendant_count` to `thread_summaries`.
--
-- Parallel to `blocking_descendant_count` but with a narrower predicate that
-- DROPS the `Running` clause: it counts transitive descendants that need
-- USER ATTENTION (WaitingForUserAnswer, or a CC thread with pending changes
-- in a workspace repo). Running children are "delegated work" — they belong
-- to ACTIVE, not REVIEW.
--
-- Used by `display_section` (`engine/thread_lifecycle.rs`) to route a thread
-- to REVIEW whenever any descendant needs attention, overriding the
-- "Running OR has_active_children → Active" arm. Without this, a parent with
-- a child paused on `CodingAgentPermissionRequest` stays in ACTIVE — the
-- child's permission card is reachable only via the child's own row.
--
-- Predicate must stay in sync with `is_attention_needing` in
-- `engine/thread_lifecycle.rs`. CTEs can't share a function across
-- migrations, so any future backfill must inline the same WHERE clause.

ALTER TABLE thread_summaries
  ADD COLUMN attention_descendant_count INT NOT NULL DEFAULT 0;

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
SET attention_descendant_count = COALESCE(sub.cnt, 0)
FROM (
    SELECT root_id,
           COUNT(*) FILTER (
               WHERE status = 'waiting_for_user_answer'
                  OR (archive_state <> 'archived'
                      AND coding_agent_proposed AND is_coding_agent
                      AND NOT coding_agent_is_external_repo)
           ) AS cnt
    FROM descendants
    GROUP BY root_id
) sub
WHERE u.thread_id = sub.root_id;
