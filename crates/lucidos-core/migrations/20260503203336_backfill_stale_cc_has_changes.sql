-- Heal thread_summaries rows where cc_has_changes drifted from reality.
--
-- Pre-2026-04-25 projection logic could leave cc_has_changes=TRUE on threads
-- whose underlying changes had already been applied or discarded. The display
-- contract surfaces such threads in Review so the user can never lose
-- unresolved work behind the archive curtain.
--
-- Conditions for healing:
--   * cc_has_changes = TRUE                  — the stale flag itself
--   * cc_is_external_repo = FALSE            — external repos legitimately
--                                              keep cc_has_changes=TRUE
--                                              without a `changes` row
--   * cc_applying = FALSE                    — leave in-flight applies alone
--   * no row in `changes` with status='pending' for this thread
--
-- We also flip status from 'waiting' back to 'idle' for healed rows, since
-- 'waiting' was being driven by the stale flag.

UPDATE thread_summaries ts
SET cc_has_changes = FALSE,
    cc_requires_restart = FALSE,
    status = CASE WHEN ts.status = 'waiting' THEN 'idle' ELSE ts.status END
WHERE ts.cc_has_changes = TRUE
  AND ts.cc_is_external_repo = FALSE
  AND ts.cc_applying = FALSE
  AND NOT EXISTS (
    SELECT 1 FROM changes c
    WHERE c.thread_id = ts.thread_id
      AND c.status = 'pending'
  );
