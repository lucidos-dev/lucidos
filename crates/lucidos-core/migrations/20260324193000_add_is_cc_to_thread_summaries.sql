-- Track whether a thread uses Claude Code, independently from the source column.
-- Needed for parent callback deduplication: CC threads emit both ResponseGenerated
-- and ClaudeCodeIdled, but only ClaudeCodeIdled should trigger the callback.
ALTER TABLE thread_summaries ADD COLUMN IF NOT EXISTS is_cc BOOLEAN NOT NULL DEFAULT FALSE;

-- Backfill: threads with SessionStarted events are CC threads
UPDATE thread_summaries SET is_cc = TRUE
WHERE thread_id IN (
    SELECT DISTINCT thread_id FROM events
    WHERE event_type IN ('SessionStarted', 'SessionResumed')
      AND thread_id IS NOT NULL
);
