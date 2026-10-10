-- Resolve a worktree's `thread-<8 hex>` directory to its thread by index.
--
-- `lookup_thread_by_short` matches `aggregate_id` on an 8-hex prefix. Under the
-- database's default collation no plain btree index can serve a prefix, so
-- without this index each lookup scans all of `events`. The Disk Usage page and
-- the hourly worktree cleanup run one lookup per worktree. `text_pattern_ops`
-- compares by byte, which is what a prefix needs. Partial on
-- `aggregate = 'thread'`, the only aggregate the lookup reads.
CREATE INDEX IF NOT EXISTS idx_events_thread_aggregate_id_pattern
    ON events (aggregate_id text_pattern_ops)
    WHERE aggregate = 'thread';
