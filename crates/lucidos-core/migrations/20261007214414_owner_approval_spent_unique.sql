-- An owner approval is spent at most once (ADR 0387). Two concurrent clause-4
-- calls may both find the same unspent approval. Only the first spend lands,
-- and the second emit fails on this index, so its caller is refused.
CREATE UNIQUE INDEX IF NOT EXISTS events_owner_approval_spent_unique
    ON events ((thread_id), ((payload->>'tool_use_id')))
    WHERE event_type = 'OwnerApprovalSpent';
