-- A device id that a hand-over replaced, and the id that replaced it.
-- Name lookups follow it, so history recorded under an old id shows the
-- device's current name. It never makes an id registered, stamped or the owner.
CREATE TABLE IF NOT EXISTS device_aliases (
    old_id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Every hand-over recorded before this table existed. A system event's fields
-- sit under `data`. The newest hand-over of an id wins.
INSERT INTO device_aliases (old_id, device_id, created_at)
SELECT DISTINCT ON (payload->'data'->>'old_device_id')
       payload->'data'->>'old_device_id',
       payload->'data'->>'device_id',
       created
FROM events
WHERE event_type = 'DeviceHandedOver'
  AND payload->'data'->>'old_device_id' IS NOT NULL
  AND payload->'data'->>'device_id' IS NOT NULL
  AND payload->'data'->>'old_device_id' <> payload->'data'->>'device_id'
ORDER BY payload->'data'->>'old_device_id', sequence DESC
ON CONFLICT (old_id) DO NOTHING;
