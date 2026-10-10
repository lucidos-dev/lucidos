-- Add `live_event_wait_count` to `thread_summaries`.
--
-- How many *event waits* the thread currently holds unresolved. Parallel to
-- `active_children_count`: both answer "this thread finished its turn but is
-- not done, because something else will wake it", and both are read as
-- `count > 0` to paint the pulsing Waiting status dot. Consumed only by the
-- frontend's `resolveVisualStatus`; it feeds NO backend predicate, so
-- `is_blocking`, `is_attention_needing`, `display_section` and
-- `available_thread_actions` are unchanged and a subscribed thread stays
-- non-blocking, non-attention-needing and archivable (ADR 0049).
--
-- Unlike `blocking_descendant_count` there is no ancestor walk: a subscription
-- belongs to the thread that registered it and nothing bubbles, so the
-- projection is a plain increment / decrement on one row in the four
-- `EventWait*` arms of `event_bus_projection_thread.rs`.
--
-- The backfill counts `EventWaitStarted` rows whose `wait_id` has no later
-- resolution on the same thread, which is the same "the event IS the wait"
-- derivation the dispatcher's boot rebuild uses. Without it, a thread already
-- watching when this lands would read Idle until its next event.

ALTER TABLE thread_summaries
  ADD COLUMN live_event_wait_count INT NOT NULL DEFAULT 0;

WITH resolved AS (
    SELECT DISTINCT e.aggregate_id, e.payload->>'wait_id' AS wait_id
    FROM events e
    WHERE e.event_type IN ('EventWaitDelivered', 'EventWaitExpired', 'EventWaitCanceled')
      AND e.payload->>'wait_id' IS NOT NULL
),
live AS (
    SELECT s.aggregate_id, COUNT(*) AS cnt
    FROM events s
    WHERE s.event_type = 'EventWaitStarted'
      AND s.payload->>'wait_id' IS NOT NULL
      AND NOT EXISTS (
          SELECT 1 FROM resolved r
          WHERE r.aggregate_id = s.aggregate_id
            AND r.wait_id = s.payload->>'wait_id'
      )
    GROUP BY s.aggregate_id
)
UPDATE thread_summaries u
SET live_event_wait_count = live.cnt
FROM live
WHERE u.thread_id::text = live.aggregate_id;
