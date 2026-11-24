-- The *cache keep-alive status* speaks one grammar: Warming, Stopped, Ended,
-- Turned off, Not kept. Two old shapes become new ones, and each says what
-- happened:
--
--   * `not_kept` with reason `turned_off` becomes the `turned_off` state. It
--     takes the pings and the cost of the run it interrupted, and the time of
--     the event.
--   * A clear (a null status) right after a run that pinged becomes that run's
--     `ended` status, dated at the clear. The saving was never measured, so
--     `saved_usd` stays absent. A clear after a run that never pinged, after a
--     stopped one, or after the user turned it off, stays null.
--
-- Then `thread_summaries.cache_keep_alive_status` takes the newest event's
-- status again, which is what its projection does from here on.
--
-- Rerunning it changes nothing: it matches only the old shapes.

CREATE TEMP TABLE keep_alive_status_rewrite ON COMMIT DROP AS
WITH statuses AS (
  SELECT id, thread_id, sequence, created, payload->'status' AS status
    FROM events
   WHERE event_type = 'CacheKeepAliveStatusChanged'
),
grouped AS (
  -- A group opens at every row that closes the run before it or starts one.
  SELECT s.*,
         count(*) FILTER (
           WHERE s.status IS NULL
              OR jsonb_typeof(s.status) = 'null'
              OR s.status->>'state' IN ('warming', 'stopped', 'ended', 'turned_off')
         ) OVER (PARTITION BY s.thread_id ORDER BY s.sequence) AS grp
    FROM statuses s
),
runs AS (
  -- The run a group's opener reports, when it pinged.
  SELECT g.*,
         first_value(CASE WHEN g.status->>'state' IN ('warming', 'stopped') THEN g.status END)
           OVER (PARTITION BY g.thread_id, g.grp ORDER BY g.sequence) AS run
    FROM grouped g
),
with_previous AS (
  SELECT r.*,
         lag(r.run) OVER (PARTITION BY r.thread_id ORDER BY r.sequence) AS previous_run,
         lag(r.status) OVER (PARTITION BY r.thread_id ORDER BY r.sequence) AS previous_status
    FROM runs r
)
SELECT w.id,
       CASE
         WHEN w.status->>'state' = 'not_kept' AND w.status->>'reason' = 'turned_off'
           THEN jsonb_build_object(
                  'state', 'turned_off',
                  'at', to_char(w.created AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
                  'beats', COALESCE(w.run->'beats', '0'::jsonb),
                  'cost_usd', COALESCE(w.run->'cost_usd', '0'::jsonb),
                  'context_tokens', COALESCE(w.run->'context_tokens', '0'::jsonb))
         WHEN jsonb_typeof(w.status) = 'null'
          AND w.previous_run->>'state' = 'warming'
          AND (w.previous_run->>'beats')::int > 0
          -- The user stopped the pings, so the run did not end while kept warm.
          AND COALESCE(w.previous_status->>'state', '') <> 'turned_off'
          AND COALESCE(w.previous_status->>'reason', '') <> 'turned_off'
           THEN jsonb_build_object(
                  'state', 'ended',
                  'since', w.previous_run->'since',
                  'at', to_char(w.created AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
                  'beats', w.previous_run->'beats',
                  'cost_usd', w.previous_run->'cost_usd')
       END AS status
  FROM with_previous w;

UPDATE events e
   SET payload = jsonb_set(e.payload, '{status}', r.status)
  FROM keep_alive_status_rewrite r
 WHERE e.id = r.id AND r.status IS NOT NULL;

UPDATE thread_summaries ts
   SET cache_keep_alive_status = latest.status
  FROM (SELECT DISTINCT ON (thread_id)
               thread_id,
               CASE WHEN jsonb_typeof(payload->'status') = 'null' THEN NULL
                    ELSE payload->'status' END AS status
          FROM events
         WHERE event_type = 'CacheKeepAliveStatusChanged'
         ORDER BY thread_id, sequence DESC) latest
 WHERE ts.thread_id = latest.thread_id
   AND ts.cache_keep_alive_status IS DISTINCT FROM latest.status;
