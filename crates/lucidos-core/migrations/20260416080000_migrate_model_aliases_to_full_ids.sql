-- Migrate short model aliases stored in event payloads to full model IDs.
-- Historical fact: during this period, "opus" was claude-opus-4-6,
-- "sonnet" was claude-sonnet-4-6, "haiku" was claude-haiku-4-5-20251001.
--
-- Applies to CCSettingsChanged, ResponseGenerated, ResponseCanceled,
-- and ResponseAborted events that carry a model field.

UPDATE events
SET payload = jsonb_set(payload, '{model}',
  CASE payload->>'model'
    WHEN 'opus'       THEN '"claude-opus-4-6"'
    WHEN 'opus[1m]'   THEN '"claude-opus-4-6[1m]"'
    WHEN 'sonnet'     THEN '"claude-sonnet-4-6"'
    WHEN 'sonnet[1m]' THEN '"claude-sonnet-4-6[1m]"'
    WHEN 'haiku'      THEN '"claude-haiku-4-5-20251001"'
  END::jsonb)
WHERE event_type IN ('CCSettingsChanged', 'ResponseGenerated', 'ResponseCanceled', 'ResponseAborted')
  AND payload->>'model' IN ('opus', 'opus[1m]', 'sonnet', 'sonnet[1m]', 'haiku');
