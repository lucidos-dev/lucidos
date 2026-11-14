-- One name, user, for what was an operator, a person or people (ADR 0655).
--
-- Stored history is corrected, not aliased: the names change, never what the
-- events record. Every statement is idempotent, so a rerun changes nothing.

-- One subscription list (`[{event_type, condition?}]`) with the old name
-- renamed, order and conditions kept.
CREATE OR REPLACE FUNCTION pg_temp.rename_user_only_subscriptions(subscriptions jsonb)
RETURNS jsonb
LANGUAGE sql
IMMUTABLE
AS $$
    SELECT jsonb_agg(
               CASE WHEN sub->>'event_type' = 'PeopleMessagePosted'
                    THEN jsonb_set(sub, '{event_type}', '"UserOnlyMessagePosted"')
                    ELSE sub
               END
               ORDER BY position
           )
    FROM jsonb_array_elements(subscriptions) WITH ORDINALITY AS t(sub, position)
$$;

-- 1. A people-only message is a user-only message.
UPDATE events
SET event_type = 'UserOnlyMessagePosted',
    payload = CASE
        WHEN payload ? 'mentioned_operator_ids'
        THEN jsonb_set(payload - 'mentioned_operator_ids', '{mentioned_user_ids}',
                       payload->'mentioned_operator_ids')
        ELSE payload
    END
WHERE event_type = 'PeopleMessagePosted';

-- 2. Trigger configs and event waits that subscribe to it, as in the
-- `PromptInjected` rename.
UPDATE events
SET payload = jsonb_set(payload, '{on}', pg_temp.rename_user_only_subscriptions(payload->'on'))
WHERE event_type IN ('TriggerCreated', 'TriggerUpdated', 'EventWaitStarted', 'EventWaitCanceled')
  AND jsonb_typeof(payload->'on') = 'array'
  AND payload->'on' @> '[{"event_type": "PeopleMessagePosted"}]';

UPDATE events
SET payload = jsonb_set(payload, '{event_type}', '"UserOnlyMessagePosted"')
WHERE event_type = 'EventWaitDelivered'
  AND payload->>'event_type' = 'PeopleMessagePosted';

UPDATE thread_summaries
SET live_event_waits = (
    SELECT jsonb_agg(
               CASE WHEN wait->'on' @> '[{"event_type": "PeopleMessagePosted"}]'
                    THEN jsonb_set(wait, '{on}', pg_temp.rename_user_only_subscriptions(wait->'on'))
                    ELSE wait
               END
               ORDER BY position
           )
    FROM jsonb_array_elements(live_event_waits) WITH ORDINALITY AS t(wait, position)
)
WHERE live_event_waits @> '[{"on": [{"event_type": "PeopleMessagePosted"}]}]';

-- 3. The user a user mention made a watcher.
UPDATE events
SET payload = jsonb_set(payload - 'operator_id', '{user_id}', payload->'operator_id')
WHERE event_type = 'WorkItemWatched'
  AND payload ? 'operator_id';

-- 4. The push group a user-only message notification joins, in the projection
-- and in the event that created it, so a replay still reads it.
UPDATE notifications
SET push_group = jsonb_set(push_group, '{kind}', '"user_only_messages"')
WHERE push_group->>'kind' = 'people_messages';

UPDATE events
SET payload = jsonb_set(payload, '{push_group,kind}', '"user_only_messages"')
WHERE event_type = 'NotificationCreated'
  AND payload->'push_group'->>'kind' = 'people_messages';

-- 5. A draft's teammate, chosen in the composer.
UPDATE thread_summaries
SET compose_selection = (compose_selection - 'person')
    || jsonb_build_object('user', compose_selection->'person')
WHERE compose_selection ? 'person';

-- 6. The user-only message delivery tables, their keys, their constraints
--    (Postgres 18 names its NOT NULL constraints too), and the notice flag.
ALTER TABLE IF EXISTS people_message_notices RENAME TO user_only_message_notices;
ALTER INDEX IF EXISTS people_message_notices_pkey RENAME TO user_only_message_notices_pkey;
ALTER INDEX IF EXISTS people_message_notices_pending RENAME TO user_only_message_notices_pending;
ALTER TABLE IF EXISTS people_message_dispatches RENAME TO user_only_message_dispatches;
ALTER INDEX IF EXISTS people_message_dispatches_pkey RENAME TO user_only_message_dispatches_pkey;
ALTER TABLE IF EXISTS people_message_push_receipts RENAME TO user_only_message_push_receipts;
ALTER INDEX IF EXISTS people_message_push_receipts_pkey RENAME TO user_only_message_push_receipts_pkey;

DO $$
DECLARE
    con record;
BEGIN
    FOR con IN
        SELECT conrelid::regclass AS tbl, conname
        FROM pg_constraint
        WHERE conname LIKE 'people\_message\_%'
    LOOP
        EXECUTE format('ALTER TABLE %s RENAME CONSTRAINT %I TO %I',
                       con.tbl, con.conname, replace(con.conname, 'people_message_', 'user_only_message_'));
    END LOOP;
END
$$;

UPDATE user_only_message_notices
SET notice = (notice - 'people_only') || jsonb_build_object('user_only', notice->'people_only')
WHERE notice ? 'people_only';
