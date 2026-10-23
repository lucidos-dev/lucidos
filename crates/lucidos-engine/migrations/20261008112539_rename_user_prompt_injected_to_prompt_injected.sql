-- Rename the `UserPromptInjected` thread event to `PromptInjected`.
--
-- The event carries `mode: ActorMode`, and agents and the engine inject with it
-- as well as people, so "User" was false. The serde alias still reads an old
-- row; it stays so the retired name is refused at registration and at emit.
-- This migration makes every SQL reader and every subscription see one name.
-- Decision: docs/adr/0389-prompt-injected-names-no-sender.md.

-- One subscription list (`[{event_type, condition?}]`) with the old name
-- renamed, order and conditions kept.
CREATE OR REPLACE FUNCTION pg_temp.rename_prompt_injected_subscriptions(subscriptions jsonb)
RETURNS jsonb
LANGUAGE sql
IMMUTABLE
AS $$
    SELECT jsonb_agg(
               CASE WHEN sub->>'event_type' = 'UserPromptInjected'
                    THEN jsonb_set(sub, '{event_type}', '"PromptInjected"')
                    ELSE sub
               END
               ORDER BY position
           )
    FROM jsonb_array_elements(subscriptions) WITH ORDINALITY AS t(sub, position)
$$;

-- 1. The rows themselves. A payload carries no serde tag (`to_payload` strips
-- it), so `type` is renamed only on an old row that still has one.
UPDATE events
SET event_type = 'PromptInjected',
    payload = CASE WHEN payload ? 'type'
                   THEN jsonb_set(payload, '{type}', '"PromptInjected"')
                   ELSE payload
              END
WHERE event_type = 'UserPromptInjected';

-- 2. Trigger configs and event waits, whose `on` list the matcher compares by
-- exact name. A trigger replays from these payloads, and a live wait rebuilds
-- from its `EventWaitStarted` at boot. A canceled wait's card names its list.
UPDATE events
SET payload = jsonb_set(payload, '{on}', pg_temp.rename_prompt_injected_subscriptions(payload->'on'))
WHERE event_type IN ('TriggerCreated', 'TriggerUpdated', 'EventWaitStarted', 'EventWaitCanceled')
  AND jsonb_typeof(payload->'on') = 'array'
  AND payload->'on' @> '[{"event_type": "UserPromptInjected"}]';

-- 3. A delivered wait names the event that matched it.
UPDATE events
SET payload = jsonb_set(payload, '{event_type}', '"PromptInjected"')
WHERE event_type = 'EventWaitDelivered'
  AND payload->>'event_type' = 'UserPromptInjected';

-- 4. The drawer's copy of each live wait.
UPDATE thread_summaries
SET live_event_waits = (
    SELECT jsonb_agg(
               CASE WHEN wait->'on' @> '[{"event_type": "UserPromptInjected"}]'
                    THEN jsonb_set(wait, '{on}', pg_temp.rename_prompt_injected_subscriptions(wait->'on'))
                    ELSE wait
               END
               ORDER BY position
           )
    FROM jsonb_array_elements(live_event_waits) WITH ORDINALITY AS t(wait, position)
)
WHERE live_event_waits @> '[{"on": [{"event_type": "UserPromptInjected"}]}]';
