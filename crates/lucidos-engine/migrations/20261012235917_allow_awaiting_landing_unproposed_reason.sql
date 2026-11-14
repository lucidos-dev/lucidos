-- A turn end now withholds new work while the thread's change waits in the
-- landing lane, with the reason 'awaiting_landing' (ADR 0463).

ALTER TABLE thread_summaries
    DROP CONSTRAINT thread_summaries_unproposed_reason_check,
    ADD CONSTRAINT thread_summaries_unproposed_reason_check
        CHECK (
            coding_agent_unproposed_reason IS NULL
            OR (
                coding_agent_change_state = 'unproposed'
                AND coding_agent_unproposed_reason IN (
                    'plan_missing', 'plan_awaiting_approval', 'outside_bound',
                    'hardening_missing', 'turn_incomplete', 'awaiting_landing'
                )
            )
        );
