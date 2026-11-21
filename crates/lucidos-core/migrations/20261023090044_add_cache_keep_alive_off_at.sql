-- Add `cache_keep_alive_off_at` to `thread_summaries`.
--
-- When the user last turned the cache keep-alive off for this one thread
-- (ADR 0554). `CacheKeepAliveTurnedOff` sets it and `CacheKeepAliveTurnedOn`
-- clears it. The switch holds while it is later than the thread's last real
-- request, so the next turn re-arms the keep-alive with no event. No
-- backfill: both events are new.
ALTER TABLE thread_summaries ADD COLUMN cache_keep_alive_off_at TIMESTAMPTZ;
