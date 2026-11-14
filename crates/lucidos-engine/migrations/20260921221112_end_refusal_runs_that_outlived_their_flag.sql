-- End every refusal run whose cause disagrees with the live enabled flag.
--
-- A run is homogeneous in its cause, and the flag is one of the two causes.
-- `WebhookStore::update` now ends the run in the statement that moves the flag,
-- so a run can no longer outlive the fact it describes. Rows written before
-- that can.
--
-- Reporting one re-attributes its count to the other cause. A hook switched off
-- after an hour of signature failures reads "every one was refused before it
-- was read", which tells its owner the secret is fine while every one of them
-- failed exactly that check.
--
-- Nothing is lost. `judge` already ignored both mismatched shapes, and
-- `last_refused_at` and `last_refusal_reason` keep the history. The next
-- delivery starts an honest run.
--
-- See docs/adr/0235-a-refused-delivery-is-an-outage.md.
UPDATE webhooks
SET refusal_run_count = 0,
    refusal_run_since = NULL,
    refusal_run_cause = NULL,
    refusal_run_reasons = '{}'::jsonb
WHERE refusal_run_cause IS NOT NULL
  AND refusal_run_cause <> CASE WHEN enabled THEN 'verification' ELSE 'disabled' END;
