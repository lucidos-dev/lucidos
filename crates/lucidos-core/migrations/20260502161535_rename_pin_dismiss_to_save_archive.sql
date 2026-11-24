-- Rename pin/dismiss → save/archive across the events table and the
-- thread_summaries projection. The UI was already updated to "Save / Saved /
-- Archive"; this migration aligns the persisted event names and column name
-- with the user-facing terminology so the wire format and storage match.

ALTER TABLE thread_summaries RENAME COLUMN is_pinned TO is_saved;
ALTER INDEX idx_thread_summaries_pinned RENAME TO idx_thread_summaries_saved;

UPDATE events SET event_type = 'ThreadSaved'    WHERE event_type = 'ThreadPinned';
UPDATE events SET event_type = 'ThreadUnsaved'  WHERE event_type = 'ThreadUnpinned';
UPDATE events SET event_type = 'ThreadArchived' WHERE event_type = 'ThreadDismissed';
