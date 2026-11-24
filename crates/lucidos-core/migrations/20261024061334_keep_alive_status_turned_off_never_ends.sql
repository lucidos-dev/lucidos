-- A run the user turned off never ends while kept warm. The one-grammar
-- migration (20261023203722) turned a clear after a run that pinged into that
-- run's `ended`, and checked for a turn-off only in the row just before the
-- clear. A turn-off followed by further `not_kept` rows slipped past it.
--
-- So an `ended` whose nearest earlier `warming` or `turned_off` status is
-- `turned_off` goes back to the clear it was. The engine never writes an
-- `ended` straight after a `turned_off`, so only migrated rows match.
--
-- Then `thread_summaries.cache_keep_alive_status` takes the newest event's
-- status again. Rerunning it changes nothing.

CREATE TEMP TABLE keep_alive_turned_off_ends ON COMMIT DROP AS
WITH statuses AS (
  SELECT id, thread_id, sequence, payload->'status'->>'state' AS state
    FROM events
   WHERE event_type = 'CacheKeepAliveStatusChanged'
),
with_run AS (
  -- The state of the newest earlier row that names a run's warmth.
  SELECT s.*,
         (array_agg(s.state) FILTER (WHERE s.state IN ('warming', 'turned_off'))
            OVER (PARTITION BY s.thread_id ORDER BY s.sequence
                  ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING)) AS earlier
    FROM statuses s
)
SELECT id
  FROM with_run
 WHERE state = 'ended'
   AND earlier[array_upper(earlier, 1)] = 'turned_off';

UPDATE events e
   SET payload = jsonb_set(e.payload, '{status}', 'null'::jsonb)
  FROM keep_alive_turned_off_ends r
 WHERE e.id = r.id;

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
