-- Speed up rebuild_missing_from_events on every engine startup. Without this,
-- both the seed scan (find change_ids in events absent from changes) and each
-- per-id replay scan (`WHERE payload->>'change_id' = $1`) fall back to a
-- sequential scan over the events table. Partial index — change-lifecycle
-- events are <1% of the total event volume.
CREATE INDEX IF NOT EXISTS events_change_id_idx
    ON events (((payload->>'change_id')))
    WHERE event_type IN (
        'ChangeProposed','ChangeApplied','ChangeDiscarded',
        'ChangeReverted','ChangeHardened',
        'MergeResolutionStarted','MergeResolutionCleared'
    );
