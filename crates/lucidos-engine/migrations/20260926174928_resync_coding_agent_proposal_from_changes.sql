-- Recompute thread_summaries.coding_agent_proposed and
-- coding_agent_requires_restart from the pending rows in `changes`.
--
-- The engine now keeps the two columns in step on every `changes` write
-- (ChangesProjection::sync_thread_proposal). A row that drifted before that
-- change would otherwise keep its stale value until its thread's next write.

UPDATE thread_summaries t
SET coding_agent_proposed = p.pending,
    coding_agent_requires_restart = p.requires_restart
FROM (
    SELECT ts.thread_id,
           COUNT(c.id) > 0 AS pending,
           COALESCE(bool_or(c.requires_restart), FALSE) AS requires_restart
    FROM thread_summaries ts
    LEFT JOIN changes c ON c.thread_id = ts.thread_id AND c.status = 'pending'
    GROUP BY ts.thread_id
) p
WHERE t.thread_id = p.thread_id
  AND (t.coding_agent_proposed, t.coding_agent_requires_restart)
      IS DISTINCT FROM (p.pending, p.requires_restart);
