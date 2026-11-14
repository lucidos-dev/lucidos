-- Drop historical ThreadMarkedRead / ThreadMarkedUnread events: the variants
-- were retired and would crash deserialization on event replay. Safe to delete
-- because the thread_summaries projection is the source of truth for section
-- state — these rows are not needed to reconstruct it.
DELETE FROM events WHERE event_type IN ('ThreadMarkedRead', 'ThreadMarkedUnread');
