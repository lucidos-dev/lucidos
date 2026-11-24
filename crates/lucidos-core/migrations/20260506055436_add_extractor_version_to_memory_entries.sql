-- Default 0 marks every existing row as "older than current" so the first
-- re_extract_stale run after this migration covers the backlog. New rows pass
-- an explicit version (see EXTRACTOR_VERSION in memory/extractor.rs).
--
-- IF EXISTS guard: on fresh installs the table is created later by
-- PgVectorIndex::init_schema (with the column already in its CREATE TABLE),
-- so the ALTER must no-op when the table doesn't exist yet.
ALTER TABLE IF EXISTS memory_entries
    ADD COLUMN IF NOT EXISTS extractor_version INTEGER NOT NULL DEFAULT 0;
