DELETE FROM events WHERE event_type IN ('DraftSaved', 'DraftDiscarded');
DROP TABLE IF EXISTS drafts;
