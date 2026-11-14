-- The owner's Allow once on an unattested caller's card runs its act once.
-- Two runs may both find the same unspent card. Only the first spend lands,
-- and the second emit fails on this index, so its act never runs.
CREATE UNIQUE INDEX IF NOT EXISTS events_unattested_act_spent_unique
    ON events ((thread_id), ((payload->>'tool_use_id')))
    WHERE event_type = 'UnattestedActSpent';
