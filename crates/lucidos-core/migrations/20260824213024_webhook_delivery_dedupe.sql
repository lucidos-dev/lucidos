-- Webhook delivery idempotency, and the headers a delivery may carry.
--
-- A sender resends. GitHub retries a slow response and offers a Redeliver
-- button, Stripe retries for days. Without a record of what already arrived,
-- every resend emits the pinned domain event again, and the bus multiplies that
-- by the number of subscribers.
--
-- `webhook_deliveries` is a nonce ledger, NOT a delivery log. It holds no
-- payload, no surface lists it, and its only reader is the next delivery. The
-- emitted domain event remains the record of what happened.
CREATE TABLE IF NOT EXISTS webhook_deliveries (
    webhook_id UUID NOT NULL REFERENCES webhooks(id) ON DELETE CASCADE,
    -- The sender's own delivery id, or a digest of the body when the hook names
    -- no header. Opaque here: nothing parses it, and it authenticates nothing.
    delivery_key TEXT NOT NULL,
    -- Who holds the claim right now. Re-minted on every claim and takeover, so
    -- an owner whose emit outran the window cannot record an event against, or
    -- delete, the claim that replaced it.
    claim_id UUID NOT NULL,
    -- The event this delivery emitted. NULL while the first copy is still
    -- emitting, and NULL again if the claim was taken over after expiry.
    event_id UUID,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- The claim. `INSERT ... ON CONFLICT` on this pair is what makes two
    -- concurrent copies of one delivery serialize, so exactly one emits.
    PRIMARY KEY (webhook_id, delivery_key)
);

-- The daily sweep deletes by age across every hook, so it needs `created`
-- indexed on its own rather than under the primary key's leading column.
CREATE INDEX IF NOT EXISTS webhook_deliveries_created_idx
    ON webhook_deliveries (created);

-- Dedupe config, in the shape `hmac` established: which header carries the
-- sender's delivery id, and how long a claim holds. NULL means the hook does
-- not dedupe, which is the default and leaves every arrival on the log.
ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS dedupe JSONB;

-- Request headers this hook copies into the event payload, under `headers`.
-- An allow-list rather than a deny-list: `Authorization` and the signature
-- header arrive on every delivery, and the events table is append-only, so a
-- carried secret is a permanent one.
ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS headers TEXT[] NOT NULL DEFAULT '{}';
