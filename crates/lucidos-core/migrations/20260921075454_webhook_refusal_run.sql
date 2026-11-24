-- The refusal run: what a webhook has been turning away since it last accepted.
--
-- `last_refusal_reason` holds only the LAST refusal, so one diagnostic probe
-- overwrites the evidence of a real outage. These three columns keep a tally
-- instead. An acceptance resets the run, and every refusal appends to it, so a
-- stray probe adds one to its own reason and leaves the rest standing.
--
-- `refusal_run_reasons` is keyed by `DeliveryRefusal::key()`, a closed set of
-- kebab-case variant names. Nothing a sender controls ever reaches it.
--
-- `refusal_run_cause` is what keeps a run HOMOGENEOUS: a refusal of the other
-- cause restarts the run rather than joining it. So the count, the start and
-- the tally always describe one fault, and a hook switched off after an hour of
-- signature failures is not reported as having thrown those away unread.
--
-- One prefix across all four, matching `RefusalRun` in Rust and `refusal_run`
-- on the wire, so the concept has one name root in every layer.
--
-- Written by `WebhookStore::{record_accepted,record_refused}`, which are
-- observations rather than decisions and emit nothing, exactly as the three
-- delivery outcome stamps beside them do.
ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS refusal_run_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS refusal_run_since TIMESTAMPTZ;
ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS refusal_run_cause TEXT;
ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS refusal_run_reasons JSONB NOT NULL DEFAULT '{}'::jsonb;

-- Seed a one-refusal run for any hook whose last delivery was turned away.
-- A workspace upgrading mid-outage should not have to rebuild its evidence from
-- scratch, and one refusal is exactly what the old columns prove.
--
-- The CASE is a snapshot of `DeliveryRefusal::reason()` as it reads today. An
-- unrecognised string leaves the tally empty rather than guessing, which costs
-- that hook nothing beyond waiting for its next real refusal.
-- A row whose reason the CASE does not recognise gets no run at all. Without a
-- cause the engine judges nothing, so seeding a count it could never classify
-- would only pin an unreadable run until the next real refusal replaced it.
UPDATE webhooks
SET refusal_run_count = 1,
    refusal_run_since = last_refused_at,
    refusal_run_cause = CASE last_refusal_reason
        WHEN 'the webhook is disabled' THEN 'disabled' ELSE 'verification' END,
    refusal_run_reasons = CASE last_refusal_reason
        WHEN 'the webhook is disabled' THEN '{"disabled": 1}'::jsonb
        WHEN 'the body is not UTF-8' THEN '{"body-not-utf8": 1}'::jsonb
        WHEN 'bearer token did not match' THEN '{"token": 1}'::jsonb
        WHEN 'signature header missing or unparseable' THEN '{"signature-missing": 1}'::jsonb
        WHEN 'signature did not match' THEN '{"signature-mismatch": 1}'::jsonb
        WHEN 'signed timestamp is too old or too far ahead'
            THEN '{"timestamp-outside-tolerance": 1}'::jsonb
        WHEN 'the configured credential does not exist' THEN '{"credential-missing": 1}'::jsonb
    END
WHERE last_refused_at IS NOT NULL
  AND (last_accepted_at IS NULL OR last_refused_at > last_accepted_at)
  AND last_refusal_reason IN (
      'the webhook is disabled',
      'the body is not UTF-8',
      'bearer token did not match',
      'signature header missing or unparseable',
      'signature did not match',
      'signed timestamp is too old or too far ahead',
      'the configured credential does not exist');
