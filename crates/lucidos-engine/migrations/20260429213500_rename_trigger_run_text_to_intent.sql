-- Rename TriggerRun::Intent field `text` → `intent` on persisted events.
-- Pairs with the serde alias added on the new field; the alias keeps replays
-- loadable across the upgrade boundary, this migration normalizes the on-disk
-- shape so all future writes match the canonical form.
-- Always strips `run.text` when present; if `run.intent` is also present
-- (mid-rollout coexistence) the existing `intent` value wins.
-- Idempotent: a row missing `run.text` falls outside the WHERE clause.

UPDATE events
SET payload = jsonb_set(
    payload #- '{run,text}',
    '{run,intent}',
    COALESCE(payload #> '{run,intent}', payload #> '{run,text}')
)
WHERE event_type IN ('TriggerCreated', 'TriggerUpdated')
  AND payload->'run' ? 'text';
