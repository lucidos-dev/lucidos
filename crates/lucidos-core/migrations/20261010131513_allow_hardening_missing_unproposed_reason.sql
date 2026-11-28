-- A turn end now withholds Lucidos-source work that never ran /harden, with
-- the reason 'hardening_missing'
-- (docs/plans/2026-10-10-plan-only-branch-skips-hardening.md).

ALTER TABLE thread_summaries
    DROP CONSTRAINT thread_summaries_unproposed_reason_check,
    ADD CONSTRAINT thread_summaries_unproposed_reason_check
        CHECK (
            coding_agent_unproposed_reason IS NULL
            OR (
                coding_agent_change_state = 'unproposed'
                AND coding_agent_unproposed_reason IN (
                    'plan_missing', 'plan_awaiting_approval', 'outside_bound',
                    'hardening_missing', 'turn_incomplete'
                )
            )
        );
