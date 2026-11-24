-- A pinned thread is never archived (ADR 0312).
--
-- `is_saved` (the pin) and `archive_state` were independent, so pinning an
-- archived thread left it both. The drawer then showed it under Pinned with no
-- way to archive it. The engine now moves a pinned thread to the inbox and
-- unpins an archived one. This migration repairs the rows written before that,
-- then makes the pair unstorable.
--
-- Repair, in this order:
--   1. A discarded draft is unpinned. It has no drawer surface to be pinned in.
--   2. Every other pinned-and-archived thread keeps its pin and moves to the
--      inbox. The pin was the user's later act, and replaying the thread's
--      events under the new rule gives the same row. So no event is owed.
--
-- `archive_state <> 'inbox'` rather than `= 'archived'`: the engine reads any
-- value but 'inbox' as archived (`ArchiveState::parse`).
UPDATE thread_summaries
   SET is_saved = FALSE
 WHERE is_saved AND archive_state <> 'inbox' AND state = 'discarded';

UPDATE thread_summaries
   SET archive_state = 'inbox'
 WHERE is_saved AND archive_state <> 'inbox';

ALTER TABLE thread_summaries DROP CONSTRAINT IF EXISTS thread_summaries_pinned_is_not_archived;
ALTER TABLE thread_summaries
    ADD CONSTRAINT thread_summaries_pinned_is_not_archived
    CHECK (NOT is_saved OR archive_state = 'inbox');
