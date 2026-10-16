-- The change summary: one model-written line saying what a change of several
-- commits does. NULL until `ChangeSummarized` lands, and cleared whenever the
-- change's description (its commit list) changes.
ALTER TABLE changes ADD COLUMN summary TEXT;

-- The per-change replay in `rebuild_one_from_events` now reads
-- `ChangeSummarized` too. A partial index only serves a query whose event-type
-- list its predicate implies, so the index takes the new type as well.
DROP INDEX IF EXISTS events_change_id_idx;
CREATE INDEX events_change_id_idx
    ON events (((payload->>'change_id')))
    WHERE event_type IN (
        'ChangeProposed','ChangeApplied','ChangeDiscarded',
        'ChangeReverted','ChangeHardened','ChangeSummarized',
        'MergeResolutionStarted','MergeResolutionCleared'
    );
