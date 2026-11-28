-- The *archivable sub-thread count*: how many of a thread's sub-threads the
-- overflow menu's "Archive N completed sub-threads" would put away. The engine
-- recounts it after commit from the archive rule itself, and the boot backfill
-- fills it for families that existed before this column.
ALTER TABLE thread_summaries
    ADD COLUMN archivable_sub_thread_count INTEGER NOT NULL DEFAULT 0;
