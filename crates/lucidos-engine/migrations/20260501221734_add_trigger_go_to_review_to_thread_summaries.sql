-- Per-thread snapshot of the firing trigger's `go_to_review` flag.
-- When TRUE, the trigger thread is treated as top-level by the section
-- transition logic so completion events surface it in REVIEW (not HISTORY).
-- Snapshotted onto the row at TriggerStarted time so a later trigger config
-- edit doesn't retroactively reroute already-completed threads.
ALTER TABLE thread_summaries
    ADD COLUMN trigger_go_to_review BOOLEAN NOT NULL DEFAULT FALSE;
