-- Successor threads: the two links, and the hold on the old thread's work.
-- Plan: docs/plans/2026-10-13-auto-compact-trial-and-successor-threads.md.

-- Projected from `SuccessorThreadSpawned` on the old thread.
ALTER TABLE thread_summaries
    ADD COLUMN successor_thread_id UUID,
    ADD COLUMN successor_of UUID;

CREATE INDEX idx_thread_summaries_successor_of
    ON thread_summaries (successor_of)
    WHERE successor_of IS NOT NULL;

-- Every new thread's first `MessageReceived` looks its own id up here.
CREATE UNIQUE INDEX idx_thread_summaries_successor_thread_id
    ON thread_summaries (successor_thread_id)
    WHERE successor_thread_id IS NOT NULL;

ALTER TABLE thread_summaries
    DROP CONSTRAINT thread_summaries_unproposed_reason_check,
    ADD CONSTRAINT thread_summaries_unproposed_reason_check
        CHECK (
            coding_agent_unproposed_reason IS NULL
            OR (
                coding_agent_change_state = 'unproposed'
                AND coding_agent_unproposed_reason IN (
                    'plan_missing', 'plan_awaiting_approval', 'outside_bound',
                    'hardening_missing', 'turn_incomplete', 'awaiting_landing',
                    'integrated_into_parent', 'carried_by_successor'
                )
            )
        );
