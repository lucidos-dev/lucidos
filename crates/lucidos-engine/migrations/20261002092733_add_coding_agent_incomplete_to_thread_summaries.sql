-- thread_summaries.coding_agent_incomplete: the thread's pending change is an
-- incomplete one, from a turn that did not finish (ADR 0346). It is not ready
-- to review, though `coding_agent_proposed` stays true so it still blocks
-- Archive. ChangesProjection::sync_thread_proposal keeps it in step with the
-- pending rows in `changes`, beside the other two proposal columns.

ALTER TABLE thread_summaries
    ADD COLUMN coding_agent_incomplete BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE thread_summaries t
SET coding_agent_incomplete = TRUE
WHERE EXISTS (
    SELECT 1 FROM changes c
    WHERE c.thread_id = t.thread_id AND c.status = 'pending' AND c.incomplete
);
