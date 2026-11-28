-- A hand-over copies the old id's gateway name to the new id (ADR 0825).
-- The copy needs the gateway, so it can fail after the rows have moved. The
-- flag keeps the copy owed until it lands. Earlier hand-overs copied the name
-- in their own transaction, so they owe nothing.
ALTER TABLE device_aliases ADD COLUMN IF NOT EXISTS name_carried BOOLEAN NOT NULL DEFAULT TRUE;
