-- Backfill the invariant that discarded rows are never in archive_state='inbox'
-- (the ThreadDiscarded projection now enforces this going forward).

UPDATE thread_summaries
SET archive_state = 'archived'
WHERE state = 'discarded' AND archive_state = 'inbox';
