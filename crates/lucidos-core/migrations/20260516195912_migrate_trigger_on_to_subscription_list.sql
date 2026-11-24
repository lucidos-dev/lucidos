-- Migrate trigger configs from the legacy single-string `on` + sibling
-- top-level `condition` shape to the new `on: [{event_type, condition}]`
-- subscription list. The reader for the legacy shape is dropped in the same
-- change, so this migration must touch every legacy row so replay produces
-- the same in-memory state under the new code.
--
-- Three steps:
--   1. Rewrite every TriggerCreated/TriggerUpdated whose `on` is a JSON
--      string into a one-entry array, inlining any sibling top-level
--      `condition`.
--   2. For TriggerUpdated rows that carried a bare `condition` (no `on`,
--      the legacy "update only the condition" partial), look up the
--      most-recent prior config with exactly one subscription and fold
--      the condition into that subscription's entry. Multi-subscription
--      or zero-subscription priors had undefined behaviour in the old
--      reader (the trigger-level `condition` only mattered when `on` was
--      set), so step 3 drops the orphan key for them.
--   3. Drop any orphan top-level `condition` key the previous steps didn't
--      consume, so the post-migration reader never sees stale fields.

-- Step 1: legacy string `on` → one-entry array.
UPDATE events
SET payload =
    (payload - 'condition' - 'on')
    || jsonb_build_object(
        'on',
        jsonb_build_array(
            jsonb_strip_nulls(jsonb_build_object(
                'event_type', payload->>'on',
                'condition', payload->'condition'
            ))
        )
    )
WHERE event_type IN ('TriggerCreated', 'TriggerUpdated')
  AND jsonb_typeof(payload->'on') = 'string';

-- Step 2: orphan `condition` on TriggerUpdated → fold into prior single
-- subscription. The JOIN reads payloads AFTER step 1 already rewrote any
-- string-shaped priors, so `prior_on->0->>'event_type'` is always populated
-- when `jsonb_array_length(prior_on) = 1`.
--
-- Both sides of the join are pre-materialized so the planner JOINs the
-- (small) event-type-filtered subsets instead of self-joining the entire
-- events table — same pattern as
-- 20260420195145_fix_trigger_invocation_event_only_backfill.sql.
WITH legacy_updates AS MATERIALIZED (
    SELECT id, created,
           payload->>'trigger_id' AS trigger_id,
           payload->'condition'   AS cond
    FROM events
    WHERE event_type = 'TriggerUpdated'
      AND payload ? 'condition'
      AND NOT payload ? 'on'
),
cfg AS MATERIALIZED (
    SELECT id, created,
           payload->>'trigger_id' AS trigger_id,
           payload->'on'          AS prior_on
    FROM events
    WHERE event_type IN ('TriggerCreated', 'TriggerUpdated')
      AND jsonb_typeof(payload->'on') = 'array'
      AND jsonb_array_length(payload->'on') = 1
),
prior_cfg AS (
    SELECT DISTINCT ON (u.id)
        u.id        AS legacy_id,
        u.cond      AS cond,
        cfg.prior_on
    FROM legacy_updates u
    JOIN cfg
      ON cfg.trigger_id = u.trigger_id
     AND cfg.created    < u.created
    -- Deterministic tiebreaker: two rows at the same NOW() (test seeds,
    -- batched emits) without `cfg.id` could pick either side.
    ORDER BY u.id, cfg.created DESC, cfg.id DESC
)
UPDATE events e
SET payload = (e.payload - 'condition') || jsonb_build_object(
    'on',
    jsonb_build_array(
        jsonb_strip_nulls(jsonb_build_object(
            'event_type', p.prior_on->0->>'event_type',
            'condition',  p.cond
        ))
    )
)
FROM prior_cfg p
WHERE e.id = p.legacy_id;

-- Step 3: any orphan `condition` left over (prior had zero or multiple
-- subscriptions) had undefined effect in the legacy code — drop it so the
-- post-migration reader doesn't carry stale data.
UPDATE events
SET payload = payload - 'condition'
WHERE event_type IN ('TriggerCreated', 'TriggerUpdated')
  AND payload ? 'condition';
