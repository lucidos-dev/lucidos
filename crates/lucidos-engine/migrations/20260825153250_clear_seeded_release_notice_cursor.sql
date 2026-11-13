-- Undo a release-notice cursor no user ever set.
--
-- The startup placement used to stamp a workspace that had ever held a thread
-- past every notice older than the release it booted on. That read `since` as
-- the release a notice shipped with, and it is a floor. The only notice
-- authored so far says 0.29.0 and shipped in 0.30.1. So every used workspace
-- was placed past it, and the modal never opened. ADR 0130 carries the rule
-- that replaced this.
--
-- Two guards keep the delete to the rows that placement wrote. Answering a
-- notice emits `ReleaseNoticeResolved` and the stamp is silent, so the event
-- tells a real answer from a stamp. A workspace with no threads is still
-- stamped on purpose, which is what keeps a modal off the first-run welcome.
DELETE FROM preferences
 WHERE key = 'release_notice_cursor'
   AND device_id IS NULL
   AND EXISTS (SELECT 1 FROM thread_summaries)
   AND NOT EXISTS (
       SELECT 1 FROM events WHERE event_type = 'ReleaseNoticeResolved'
   );
