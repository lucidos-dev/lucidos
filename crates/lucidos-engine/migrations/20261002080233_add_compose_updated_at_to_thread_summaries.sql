-- When this thread's compose draft was last written. Every applied
-- `PUT /api/v1/threads/:id/compose` stamps it. That PUT is the only writer of
-- non-empty draft text, so the stamp dates any draft the row holds.
--
-- The `threads` tool's `drafts` action reports it as `last_edited`, so the
-- agent can say which draft the user touched last.
--
-- No backfill. NULL means the draft was written before this column existed,
-- and the agent reads that as "unknown" rather than a guessed date.
ALTER TABLE thread_summaries
    ADD COLUMN IF NOT EXISTS compose_updated_at TIMESTAMPTZ NULL;
