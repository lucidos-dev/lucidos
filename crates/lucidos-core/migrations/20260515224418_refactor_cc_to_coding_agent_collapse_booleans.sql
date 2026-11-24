-- Rename the `cc_*` boolean cluster on `thread_summaries` to `coding_agent_*`
-- to match the `CodingAgent*` event naming, and split the conflated
-- "has changes" concept:
--   `cc_has_changes`  → `coding_agent_proposed`   (set only by ChangeProposed,
--                                                  drives Apply / Discard)
--   `branch_has_diff` → `coding_agent_has_diff`   (pure git truth — `git diff
--                                                  main..branch` non-empty —
--                                                  drives the Diff button)

ALTER TABLE thread_summaries RENAME COLUMN cc_has_changes      TO coding_agent_proposed;
ALTER TABLE thread_summaries RENAME COLUMN cc_requires_restart TO coding_agent_requires_restart;
ALTER TABLE thread_summaries RENAME COLUMN cc_is_external_repo TO coding_agent_is_external_repo;
ALTER TABLE thread_summaries RENAME COLUMN cc_applying         TO coding_agent_applying;
ALTER TABLE thread_summaries RENAME COLUMN branch_has_diff     TO coding_agent_has_diff;
