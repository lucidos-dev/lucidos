-- The custom folder a pinned thread sits in. NULL means the plain Pinned
-- section. `ThreadMovedToFolder` writes it, and unpin, archive and discard
-- clear it. No event carried a folder before this column, so nothing backfills.
ALTER TABLE thread_summaries ADD COLUMN IF NOT EXISTS pin_folder TEXT;

ALTER TABLE thread_summaries DROP CONSTRAINT IF EXISTS thread_summaries_folder_is_pinned;
ALTER TABLE thread_summaries
    ADD CONSTRAINT thread_summaries_folder_is_pinned
    CHECK (pin_folder IS NULL OR is_saved);
