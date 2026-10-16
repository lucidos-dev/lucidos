-- Rename inbox_status to section on thread_summaries.
-- Section values: 'default' (history/pinned), 'unread', 'waiting'.
-- The frontend uses this as the initial section before events load.

ALTER TABLE thread_summaries RENAME COLUMN inbox_status TO section;

-- Update 'none' values to 'default' and fix column default for new rows
UPDATE thread_summaries SET section = 'default' WHERE section = 'none';
ALTER TABLE thread_summaries ALTER COLUMN section SET DEFAULT 'default';

-- Drop old index and create new one with renamed column
DROP INDEX IF EXISTS idx_thread_summaries_inbox;
CREATE INDEX IF NOT EXISTS idx_thread_summaries_section
    ON thread_summaries (section, last_activity DESC)
    WHERE section != 'default';
