-- thread_summaries.coding_agent_bg_bash_pending — true when the CC turn
-- ended idle but a background bash task was still running (engine-side
-- run_bash_background or CC's own Bash{run_in_background:true}). The
-- propose-side gate (`should_propose_change_at_idle`) refuses to emit
-- ChangeProposed while either flag holds, so without this column the
-- projection cannot distinguish "truly idle, no diff" from "idled waiting
-- on bg-bash". The frontend reads it to render "CC waiting on background
-- tasks" instead of dropping the thread into REVIEW with broken
-- affordances (the original "no Apply button" bug).
--
-- Set by the CodingAgentIdled projection from the event payload's
-- `bg_bash_pending` field; the next CodingAgentIdled overwrites it (when
-- bg-bash drains, the same flag arrives as false). The safety-net
-- BgBashWakeRequested timer guarantees an idle re-fires within 5 min.
ALTER TABLE thread_summaries
    ADD COLUMN coding_agent_bg_bash_pending BOOLEAN NOT NULL DEFAULT FALSE;
