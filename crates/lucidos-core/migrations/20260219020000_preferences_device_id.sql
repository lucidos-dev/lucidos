-- Add device_id column to preferences for per-device settings.
-- Global preferences have device_id IS NULL; per-device ones have the device UUID.

ALTER TABLE preferences DROP CONSTRAINT preferences_pkey;

ALTER TABLE preferences ADD COLUMN device_id TEXT;

-- Unique index using COALESCE so NULL device_id is treated as '' for uniqueness
CREATE UNIQUE INDEX preferences_key_device_unique
  ON preferences (key, COALESCE(device_id, ''));
