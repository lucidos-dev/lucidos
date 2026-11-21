-- Rename `is_cc` to `is_coding_agent` to match the role-level question the
-- column asks ("is this thread driven by a coding agent?"). Claude Code is
-- the only product today; a future coding agent (e.g. Codex) would share the
-- same column.
ALTER TABLE thread_summaries RENAME COLUMN is_cc TO is_coding_agent;
