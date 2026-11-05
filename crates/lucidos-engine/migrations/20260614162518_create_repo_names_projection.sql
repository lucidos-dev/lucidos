-- Durable repo-name projection. The live `repositories` table is a mutable
-- config table: when a repo is removed (`DELETE FROM repositories`), its name
-- is gone, so any thread that referenced it could only show the raw UUID in the
-- filter / thread rows. The `RepositoryAdded` event already carries the name
-- (it's the source of truth), but the read path resolved names via a live
-- subquery against `repositories` instead of the event log — classic
-- event-sourcing drift: the events know the name, the read model forgot it.
--
-- This projection retains the name for every repo ever added, surviving the
-- repo's removal. Maintained by EventBus from `RepositoryAdded` (upsert);
-- `RepositoryRemoved` deliberately does NOT touch it (retention is the point).
-- The read path resolves `cc_repo_name` as COALESCE(live repositories, this) so
-- live repos still show their current name and deleted repos show the last
-- known one.
CREATE TABLE IF NOT EXISTS repo_names (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL
);

-- Backfill from the event log (source of truth) AND the current registry, so
-- already-deleted repos recover their historical name immediately on upgrade.
-- Per id, the live registry name wins (priority 2 — it's the current truth,
-- including any rename applied via the upsert path that doesn't re-emit an
-- event); otherwise the most-recent `RepositoryAdded` name wins. The regex
-- guards the `::uuid` cast against any malformed payload.
INSERT INTO repo_names (id, name)
SELECT DISTINCT ON (id) id, name
FROM (
    SELECT id, name, 2 AS priority, created_at AS ts
    FROM repositories
    UNION ALL
    SELECT (payload->>'repo_id')::uuid AS id,
           payload->>'name' AS name,
           1 AS priority,
           created AS ts
    FROM events
    WHERE event_type = 'RepositoryAdded'
      AND payload->>'name' IS NOT NULL
      AND payload->>'repo_id' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
) candidates
ORDER BY id, priority DESC, ts DESC
ON CONFLICT (id) DO NOTHING;
