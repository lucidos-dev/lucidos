-- Whether an engine whose environment opts out of usage telemetry has run
-- since the cursor last moved.
--
-- An opted-out engine sets it at startup, before any model call, and on each
-- pass. Captures it made after its last pass then stay after the cursor. The
-- next pass without the opt-out skips everything up to the horizon and clears
-- the flag in the same UPDATE, so none of them is ever sent.

ALTER TABLE claude_gateway_telemetry_cursor
    ADD COLUMN IF NOT EXISTS opted_out BOOLEAN NOT NULL DEFAULT FALSE;
