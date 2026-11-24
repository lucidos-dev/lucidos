-- The name a device was given when it paired with the workspace gateway.
--
-- The gateway forwards it on every request it proxies. The engine stores it so
-- a device nobody renamed is called by that name everywhere, instead of
-- `device-<first 8>`. A typed `name` still wins.
ALTER TABLE devices ADD COLUMN IF NOT EXISTS pairing_label TEXT;
