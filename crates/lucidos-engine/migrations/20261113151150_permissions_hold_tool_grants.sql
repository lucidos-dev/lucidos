-- The permissions table becomes the authority for tool grants: every line of
-- the three grant files, and every "Allow for this thread" a tool card stores.
-- The files stay as a mirror the engine rewrites.

-- A grant may come from no thread (the boot import, Settings), and a standing
-- grant outlives the thread it was granted in.
ALTER TABLE permissions ALTER COLUMN granted_in_thread_id DROP NOT NULL;
ALTER TABLE permissions DROP CONSTRAINT IF EXISTS permissions_granted_in_thread_id_fkey;
ALTER TABLE permissions ADD CONSTRAINT permissions_granted_in_thread_id_fkey
    FOREIGN KEY (granted_in_thread_id) REFERENCES thread_summaries (thread_id)
    ON DELETE SET NULL;
ALTER TABLE permissions ALTER COLUMN request_tool_use_id DROP NOT NULL;

-- The grant file line a tool grant stores, verbatim. Its lane's own matcher
-- reads it, so a line keeps exactly the meaning it had in the file.
ALTER TABLE permissions ADD COLUMN IF NOT EXISTS pattern TEXT;

-- Where a permission came from.
ALTER TABLE permissions ADD COLUMN IF NOT EXISTS source TEXT NOT NULL DEFAULT 'card'
    CHECK (source IN (
        'card', 'batch', 'legacy-import', 'settings', 'file-review', 'file-edit', 'revoke'
    ));

UPDATE permissions p SET source = 'batch'
FROM events e
WHERE e.event_type = 'PermissionGranted'
  AND e.payload->>'permission_id' = p.permission_id::text
  AND e.payload->'picked_by'->>'kind' = 'batch';

-- One live row per line and holder.
CREATE UNIQUE INDEX IF NOT EXISTS permissions_live_pattern
    ON permissions (action, pattern, holder_kind, (COALESCE(holder_id, '')))
    WHERE ended_at IS NULL AND pattern IS NOT NULL;

-- Each grant file is imported once. A system event's fields sit under `data`.
CREATE UNIQUE INDEX IF NOT EXISTS events_grant_file_imported_unique
    ON events (((payload->'data'->>'grant_file')))
    WHERE event_type = 'PermissionGrantsChanged'
      AND payload->'data'->>'source' = 'legacy-import';
