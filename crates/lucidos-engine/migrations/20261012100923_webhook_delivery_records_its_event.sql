-- A delivery's ledger row now commits in its event's own transaction (ADR 0444),
-- so it always carries the event id. A row with none is a claim a dead handler
-- stranded under the old scheme. Dropping it frees the key for the resend.
DELETE FROM webhook_deliveries WHERE event_id IS NULL;
ALTER TABLE webhook_deliveries DROP COLUMN claim_id;
ALTER TABLE webhook_deliveries ALTER COLUMN event_id SET NOT NULL;
