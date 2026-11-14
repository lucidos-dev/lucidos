-- Sidebar redesign Phase 1 (naming-only): rename the thread_summaries.section
-- column to archive_state and migrate values default->archived, unread->inbox.
-- See docs/plans/2026-05-01-sidebar-redesign-design.md and -impl.md.
--
-- Wire JSON field stays "section" via a SELECT alias in core/store/threads.rs;
-- only the database column and stored values change here.

ALTER TABLE thread_summaries RENAME COLUMN section TO archive_state;

UPDATE thread_summaries SET archive_state = CASE
    WHEN archive_state = 'default' THEN 'archived'
    WHEN archive_state = 'unread'  THEN 'inbox'
    ELSE archive_state
END;

ALTER TABLE thread_summaries ALTER COLUMN archive_state SET DEFAULT 'archived';
