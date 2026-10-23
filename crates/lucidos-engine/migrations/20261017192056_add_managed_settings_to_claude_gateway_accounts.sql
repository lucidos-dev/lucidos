-- The gateway's managed settings for the stored sign-in, fetched from
-- `GET {issuer}/managed/settings`.
--
-- They live on the account row, so a restart keeps them and a sign-out drops
-- them. `managed_settings` is NULL when the gateway has no policy for this
-- user (it answered 404) or none has been fetched yet; `managed_settings_fetched_at`
-- tells the two apart. `managed_settings_etag` is sent back as `If-None-Match`.

ALTER TABLE claude_gateway_accounts
    ADD COLUMN IF NOT EXISTS managed_settings JSONB,
    ADD COLUMN IF NOT EXISTS managed_settings_etag TEXT,
    ADD COLUMN IF NOT EXISTS managed_settings_fetched_at TIMESTAMPTZ;
