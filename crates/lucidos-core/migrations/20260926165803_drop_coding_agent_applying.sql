-- Drop thread_summaries.coding_agent_applying.
--
-- MergeConflictDetected set it and ChangeApplyFailed cleared it, but nothing
-- acted on the value. It also went stale: MergeResolutionCleared and the boot
-- stale-merge cleanup never cleared it.

ALTER TABLE thread_summaries DROP COLUMN coding_agent_applying;
