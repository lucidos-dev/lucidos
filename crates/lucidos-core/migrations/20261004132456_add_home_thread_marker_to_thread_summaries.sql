-- thread_summaries.is_home: this row is the workspace's *home thread* (ADR 0362).
-- `HomeThreadCreated` writes it, and nothing ever clears it.
--
-- Three constraints make the wrong states unstorable:
--   1. At most one home thread per workspace: a partial unique index.
--   2. The home thread is never archived, on any path.
--   3. The home thread is never a sub-thread, so no cascade reaches it from a
--      parent. Archive and delete refuse it as a target in the engine.

ALTER TABLE thread_summaries
    ADD COLUMN is_home BOOLEAN NOT NULL DEFAULT FALSE;

CREATE UNIQUE INDEX thread_summaries_one_home_thread
    ON thread_summaries (is_home) WHERE is_home;

ALTER TABLE thread_summaries
    ADD CONSTRAINT thread_summaries_home_is_not_archived
    CHECK (NOT is_home OR archive_state = 'inbox');

ALTER TABLE thread_summaries
    ADD CONSTRAINT thread_summaries_home_has_no_parent
    CHECK (NOT is_home OR parent_thread_id IS NULL);
