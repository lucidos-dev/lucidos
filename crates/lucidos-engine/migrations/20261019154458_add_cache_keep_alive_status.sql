-- Add `cache_keep_alive_status` to `thread_summaries`.
--
-- The *cache keep-alive status*: what the keep-alive does for a waiting
-- thread, as its tick last saw it. The `CacheKeepAliveStatusChanged` event
-- projects into it, and NULL means the thread has nothing to keep warm. No
-- backfill: the first tick after boot writes every live status.
ALTER TABLE thread_summaries ADD COLUMN cache_keep_alive_status JSONB;
