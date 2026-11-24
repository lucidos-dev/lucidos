-- Option B for the CC review-required state: when a CC session ends with a
-- proposed change, the child settles to status='idle' (not 'waiting').
-- `waiting` previously did double duty as "CC has changes to review", but
-- semantically the loop's *work* is done — the diff is an artifact, not a
-- parked loop. Reserves `waiting` for legacy historical rows only;
-- `waiting_for_user_answer` continues to mean "loop is parked for human
-- input" (clause 1 of `is_blocking` in thread_lifecycle.rs).
--
-- The frontend's `is_blocking` predicate (clause 3) still flags
-- `coding_agent_proposed && CodingAgent && !is_external` as blocking
-- regardless of status, so `blocking_descendant_count` semantics don't
-- change.
--
-- Backfill: every CC thread currently sitting at 'waiting' (whether
-- coding_agent_proposed=true or stuck waiting without it — the startup
-- orphan sweep in main.rs:325 covers the latter, but inline here for the
-- migration to be self-contained).
UPDATE thread_summaries
SET status = 'idle'
WHERE source = 'claude_code'
  AND status = 'waiting';

-- Reconcile active_children_count from running children once the statuses
-- above settle. Mirrors main.rs::reconcile_active_children_count (which
-- also runs on every startup). Without this, parents whose CC child just
-- flipped from 'waiting' to 'idle' could keep a stale non-zero count from
-- the runtime drift this migration is paired with fixing.
WITH running_child_counts AS (
    SELECT parent_thread_id, COUNT(*) AS cnt
    FROM thread_summaries
    WHERE parent_thread_id IS NOT NULL AND status = 'running'
    GROUP BY parent_thread_id
),
parents AS (
    SELECT DISTINCT parent_thread_id AS thread_id
    FROM thread_summaries WHERE parent_thread_id IS NOT NULL
)
UPDATE thread_summaries p
SET active_children_count = COALESCE(rc.cnt, 0)::int
FROM parents pa LEFT JOIN running_child_counts rc
     ON rc.parent_thread_id = pa.thread_id
WHERE p.thread_id = pa.thread_id
  AND p.active_children_count != COALESCE(rc.cnt, 0)::int;
