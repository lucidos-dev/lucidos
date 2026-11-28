-- The gateway owns device identity (ADR 0825).
--
-- The names move to a holding table first. The engine hands each row to the
-- gateway at boot and deletes it only once the gateway has it, so no name is
-- lost to a gateway that is down while this runs.
CREATE TABLE IF NOT EXISTS device_name_imports (
    device_id TEXT PRIMARY KEY,
    name TEXT,
    pairing_label TEXT
);

INSERT INTO device_name_imports (device_id, name, pairing_label)
SELECT id, NULLIF(btrim(name), ''), NULLIF(btrim(pairing_label), '')
FROM devices
WHERE btrim(COALESCE(name, '')) <> '' OR btrim(COALESCE(pairing_label, '')) <> ''
ON CONFLICT (device_id) DO NOTHING;

ALTER TABLE devices DROP COLUMN IF EXISTS name;
ALTER TABLE devices DROP COLUMN IF EXISTS pairing_label;
