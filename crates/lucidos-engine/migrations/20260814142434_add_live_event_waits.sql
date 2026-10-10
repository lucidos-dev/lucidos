-- Add `live_event_waits` to `thread_summaries`.
--
-- The live *event waits* the thread holds unresolved, in full: one object per
-- wait carrying `wait_id`, `on`, `reason` and `expires_at`. The count column
-- beside it says HOW MANY, and this says WHICH, which is everything the
-- subscription indicator renders (the reason, the countdown, the Stop waiting
-- button).
--
-- It exists because the client used to build that list ONLY by folding a
-- thread's own `EventWait*` events. Nothing reconciled it against the server,
-- so one missed `EventWaitDelivered` stranded a resolved wait on screen
-- forever, with a live countdown to a deadline nobody was waiting for. The
-- count self-healed on every snapshot and the list could not, and the two
-- disagreeing is exactly what the bug looked like.
--
-- **Written in the same UPDATE as `live_event_wait_count`, in all four
-- `EventWait*` arms of `event_bus_projection_thread.rs`.** That pairing is the
-- point: the two move under one statement, inside one transaction, or neither
-- moves, so they cannot disagree again. The arm filters by `wait_id` before
-- appending, so a replayed start is idempotent rather than a duplicate entry;
-- each resolution filters by `wait_id` and is a no-op when the wait is absent,
-- which is the list's counterpart to the count's `GREATEST(0, ...)` floor.
--
-- Read by the frontend's subscription indicator via `ThreadSummary` and
-- `ThreadAggregate`. No backend predicate reads it, exactly as with the count:
-- a subscribed thread stays non-blocking, non-attention-needing and archivable
-- (ADR 0049).
--
-- The backfill mirrors `LIVE_WAITS_SQL` in `engine/event_wait/mod.rs`, the
-- dispatcher's own boot rebuild and the authority on which waits are live: an
-- `EventWaitStarted` with no LATER resolution carrying the same `wait_id` on
-- the same thread. Without a backfill, a thread already watching when this
-- lands would show a blank panel until its next `EventWait*` event.
--
-- **`r.sequence > s.sequence` and `aggregate = 'thread'` are both taken from
-- that query rather than from the count's own migration**, which has neither. A
-- resolution older than the start does not resolve it, so an unordered match
-- writes a blank panel for a wait the dispatcher is holding live. The
-- `aggregate` scope is the same guard `LIVE_WAITS_SQL` documents: `aggregate_id`
-- carries a thread uuid only for thread events, and on a DOMAIN event it
-- carries the event type name, which an untrusted emit endpoint can choose.
--
-- It rewrites `live_event_wait_count` from that one derivation, so the two
-- agree from the instant the column exists rather than from the thread's next
-- event. One entry is listed only when it has an `expires_at` to count down to,
-- which the count's migration did not require. Such a row cannot deserialize
-- back into an `EventWaitStarted` either, so the dispatcher never held it as a
-- live wait. Every thread that ever armed a wait is visited (the LEFT JOIN), so
-- a thread whose only entries were dropped that way lands on a truthful zero
-- rather than keeping a count nothing can explain.

ALTER TABLE thread_summaries
  ADD COLUMN live_event_waits JSONB NOT NULL DEFAULT '[]'::jsonb;

WITH live AS (
    SELECT s.aggregate_id,
           jsonb_agg(
               jsonb_build_object(
                   'wait_id', s.payload->>'wait_id',
                   'on', COALESCE(s.payload->'on', '[]'::jsonb),
                   'reason', COALESCE(s.payload->>'reason', ''),
                   'expires_at', s.payload->>'expires_at'
               )
               ORDER BY s.sequence
           ) AS waits
    FROM events s
    WHERE s.aggregate = 'thread'
      AND s.event_type = 'EventWaitStarted'
      AND s.payload->>'wait_id' IS NOT NULL
      AND s.payload->>'expires_at' IS NOT NULL
      AND NOT EXISTS (
          SELECT 1 FROM events r
          WHERE r.aggregate_id = s.aggregate_id
            AND r.sequence > s.sequence
            AND r.event_type IN ('EventWaitDelivered', 'EventWaitExpired', 'EventWaitCanceled')
            AND r.payload->>'wait_id' = s.payload->>'wait_id'
      )
    GROUP BY s.aggregate_id
),
ever_armed AS (
    SELECT DISTINCT e.aggregate_id
    FROM events e
    WHERE e.aggregate = 'thread'
      AND e.event_type = 'EventWaitStarted'
)
UPDATE thread_summaries u
SET live_event_waits = COALESCE(live.waits, '[]'::jsonb),
    live_event_wait_count = jsonb_array_length(COALESCE(live.waits, '[]'::jsonb))
FROM ever_armed
LEFT JOIN live ON live.aggregate_id = ever_armed.aggregate_id
WHERE u.thread_id::text = ever_armed.aggregate_id;
