-- An integrated sub-thread folds into its parent's branch. It proposes
-- nothing, so its hardened work waits with the reason
-- 'integrated_into_parent' until the parent's agent merges it.

ALTER TABLE thread_summaries
    ADD COLUMN coding_agent_integrated BOOLEAN NOT NULL DEFAULT FALSE;

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
                    'integrated_into_parent'
                )
            )
        );
