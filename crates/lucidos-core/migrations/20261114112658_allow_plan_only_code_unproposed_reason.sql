-- A plan-only approval cannot land code. Persist that refusal alongside
-- every other proposal hold rather than rejecting its projection write.
ALTER TABLE thread_summaries
    DROP CONSTRAINT thread_summaries_unproposed_reason_check,
    ADD CONSTRAINT thread_summaries_unproposed_reason_check
        CHECK (
            coding_agent_unproposed_reason IS NULL
            OR (
                coding_agent_change_state = 'unproposed'
                AND coding_agent_unproposed_reason IN (
                    'plan_missing', 'plan_awaiting_approval', 'plan_only_code', 'outside_bound',
                    'hardening_missing', 'turn_incomplete', 'awaiting_landing',
                    'integrated_into_parent', 'carried_by_successor'
                )
            )
        );
