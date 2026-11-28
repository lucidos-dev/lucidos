-- What each webhook's last delivery did, so silence can be read.
--
-- "Arrived and was refused" and "never arrived" produce the same symptom today,
-- no events, and have completely different causes. A rotated secret looks
-- exactly like a dead ingress. These three columns are the only thing that
-- tells them apart.
--
-- Two are not what they look like:
--   * The stamps are observations, not decisions. Nothing emits when they
--     change, and `core/announced_surfaces.rs` records that exemption.
--   * `last_refusal_reason` is a log string from `DeliveryRefusal::reason()`.
--     It is shown to the workspace owner on the Webhooks page and is never
--     returned to a sender, because a public endpoint that says WHY it refused
--     is a hint to whoever is guessing.
ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS last_accepted_at TIMESTAMPTZ;
ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS last_refused_at TIMESTAMPTZ;
ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS last_refusal_reason TEXT;
