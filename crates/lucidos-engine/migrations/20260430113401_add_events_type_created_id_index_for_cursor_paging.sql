-- Cursor-paged event queries (`/api/events/query?before_event_id=...`) order
-- by `(created DESC, id DESC)` and filter on the same lexicographic key.
-- The pre-existing `idx_events_type_created` is `(event_type, created ASC)`,
-- which lets Postgres scan backwards for `created` but still requires a sort
-- on `id` per page — dominant cost for high-cardinality event types like
-- BrowserLearningObserved.
--
-- This index matches the new ORDER BY + cursor predicates exactly, so paging
-- through a million-event type can stream the LIMIT without a sort.
CREATE INDEX IF NOT EXISTS idx_events_type_created_id
    ON events (event_type, created DESC, id DESC);
