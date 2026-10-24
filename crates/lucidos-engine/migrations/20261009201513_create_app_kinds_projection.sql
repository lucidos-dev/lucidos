-- The kind of each removed app folder (ADR 0404). A widget's kind lives only in
-- its manifest, which leaves with the folder. The thread filter still lists a
-- removed widget, and reads its kind here.
--
-- Maintained by EventBus from `AppDeleted` (upsert). No backfill: `AppDeleted`
-- was broadcast-only until this table existed, so the event log holds no row.
CREATE TABLE IF NOT EXISTS app_kinds (
    app_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL
);
