-- Fix the over-eager invocation backfill from migration 20260420164143.
--
-- That migration defaulted EVERY legacy TriggerStarted row to
--   {"kind":"Schedule"}
-- on the assumption it couldn't tell from the row alone whether a past run
-- was schedule-fired or event-fired. We can in fact tell, by joining the
-- trigger config that was in force at the time of the run: a trigger with
-- an `on` event subscription and an empty `schedule` array is event-only,
-- so any historical run of it must have been event-fired.
--
-- Scoping rules:
--   * Only touch rows created BEFORE the bad migration ran. New rows have
--     correct invocations set by the emit code and must be left alone.
--   * Only promote runs whose trigger config at the time of firing was
--     event-only (`on IS NOT NULL` AND empty schedule). Schedule-only and
--     hybrid configs stay as Schedule — for hybrid we genuinely cannot tell
--     which path fired the run, and Schedule is the safer guess.
--   * Use the latest TriggerCreated/TriggerUpdated whose `created` is at or
--     before the TriggerStarted's `created`, so triggers later repurposed
--     don't retroactively rewrite past runs.

WITH bad_migration AS (
    SELECT installed_on FROM _sqlx_migrations WHERE version = 20260420164143
),
-- Pre-filter both sides of the self-join. Without these the planner can't
-- avoid scanning the full events table per side (no index on event_type).
started AS MATERIALIZED (
    SELECT id, created, payload
    FROM events, bad_migration
    WHERE event_type = 'TriggerStarted'
      AND created < bad_migration.installed_on
      AND payload->'invocation' = '{"kind":"Schedule"}'::jsonb
),
cfg AS MATERIALIZED (
    SELECT created, payload
    FROM events
    WHERE event_type IN ('TriggerCreated', 'TriggerUpdated')
),
config_at_run AS (
    SELECT DISTINCT ON (started.id)
        started.id           AS started_id,
        cfg.payload->>'on'   AS on_event,
        cfg.payload->'schedule' AS schedule
    FROM started
    JOIN cfg
      ON (
            -- Modern rows store the real trigger_id on both sides.
            cfg.payload->>'trigger_id' = COALESCE(
                started.payload->>'trigger_id',
                started.payload->>'task_id'
            )
            -- Legacy rows used a derived hash for task_id that doesn't
            -- line up with the trigger config's UUID, so fall back to
            -- name matching. Same-named configs are disambiguated by
            -- "latest before run" via DISTINCT ON below. If a user ever
            -- deleted a trigger and recreated an unrelated one under the
            -- same name, runs of the original may be reclassified using
            -- the recreated trigger's config — undefined for legacy data.
            OR cfg.payload->>'name' = COALESCE(
                started.payload->>'trigger_name',
                started.payload->>'task_name'
            )
         )
     AND cfg.created <= started.created
    ORDER BY started.id, cfg.created DESC
)
UPDATE events e
SET payload = jsonb_set(
    e.payload,
    '{invocation}',
    jsonb_build_object('kind', 'Event', 'event_type', c.on_event)
)
FROM config_at_run c
WHERE e.id = c.started_id
  AND c.on_event IS NOT NULL
  AND c.schedule IS NOT NULL
  AND jsonb_array_length(c.schedule) = 0;
