-- Clear orphan "change proposed" chips left by the old per-commit emit.
--
-- The deleted post-commit hook fired `ChangeProposed` per commit, and the
-- projection flipped `coding_agent_proposed = TRUE` on each. If CC died
-- before the end-of-turn aggregate emit landed a `changes` row, the chip
-- stuck on with nothing backing it. New emit logic only flips the chip on
-- the aggregate, so no new orphans can form — this is a one-shot sweep
-- for the rows that pre-date the fix.
--
-- Healing condition: chip set, but no pending row in `changes`. External
-- repos legitimately keep the chip without a `changes` row (the runtime
-- never proposes for them), so exclude them.

UPDATE thread_summaries ts
SET coding_agent_proposed = FALSE,
    coding_agent_requires_restart = FALSE,
    status = CASE WHEN ts.status = 'waiting' THEN 'idle' ELSE ts.status END
WHERE ts.coding_agent_proposed = TRUE
  AND ts.coding_agent_is_external_repo = FALSE
  AND ts.coding_agent_applying = FALSE
  AND NOT EXISTS (
    SELECT 1 FROM changes c
    WHERE c.thread_id = ts.thread_id
      AND c.status = 'pending'
  );
