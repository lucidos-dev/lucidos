-- thread_summaries.coding_agent_change_state replaces three booleans with one
-- state (ADR 0400): 'none', 'unproposed' or 'proposed'.
-- coding_agent_unproposed_reason says why a turn end withheld unproposed work,
-- and coding_agent_requires_restart now means something only on 'proposed'.
--
-- The backfill maps the old pair exactly: proposed stays proposed, a diff with
-- no proposal is unproposed, anything else is none. No row carries a reason
-- yet; the boot held-back sweep announces each withheld branch with one.

ALTER TABLE thread_summaries
    ADD COLUMN coding_agent_change_state TEXT NOT NULL DEFAULT 'none',
    ADD COLUMN coding_agent_unproposed_reason TEXT;

UPDATE thread_summaries
SET coding_agent_change_state = CASE
        WHEN coding_agent_proposed THEN 'proposed'
        ELSE 'unproposed'
    END
WHERE coding_agent_proposed OR coding_agent_has_diff;

UPDATE thread_summaries
SET coding_agent_requires_restart = FALSE
WHERE coding_agent_requires_restart AND coding_agent_change_state <> 'proposed';

ALTER TABLE thread_summaries
    ADD CONSTRAINT thread_summaries_change_state_check
        CHECK (coding_agent_change_state IN ('none', 'unproposed', 'proposed')),
    ADD CONSTRAINT thread_summaries_unproposed_reason_check
        CHECK (
            coding_agent_unproposed_reason IS NULL
            OR (
                coding_agent_change_state = 'unproposed'
                AND coding_agent_unproposed_reason IN (
                    'plan_missing', 'plan_awaiting_approval', 'outside_bound', 'turn_incomplete'
                )
            )
        ),
    ADD CONSTRAINT thread_summaries_requires_restart_check
        CHECK (NOT coding_agent_requires_restart OR coding_agent_change_state = 'proposed');

ALTER TABLE thread_summaries
    DROP COLUMN coding_agent_has_diff,
    DROP COLUMN coding_agent_proposed,
    DROP COLUMN coding_agent_incomplete;
