-- When the gateway's managed settings started asking for usage telemetry,
-- for the stored sign-in.
--
-- The exporter sends a capture only when it was made at or after this time,
-- so a capture made while telemetry was off is never sent later. NULL means
-- telemetry is off, or no policy has been fetched yet. The first policy after a
-- sign-in that asks for telemetry applies from the sign-in's `created_at`.

ALTER TABLE claude_gateway_accounts
    ADD COLUMN IF NOT EXISTS managed_telemetry_since TIMESTAMPTZ;
