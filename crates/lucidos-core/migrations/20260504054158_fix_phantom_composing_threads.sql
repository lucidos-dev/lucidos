-- Real drafts always set `compose_mode` (carried by ThreadStarted). A row
-- with `state='composing' AND compose_mode IS NULL` therefore can't be a
-- draft — it's a pre-existing thread that the prior compose-state migration
-- mis-classified because its lifecycle never emitted `MessageReceived`.
UPDATE thread_summaries
SET state = 'active'
WHERE state = 'composing' AND compose_mode IS NULL;

-- Only `ThreadStarted` legitimately produces a draft, and it sets
-- `state='composing'` explicitly. Every other insert path represents prior
-- activity, so the default must be `'active'`.
ALTER TABLE thread_summaries ALTER COLUMN state SET DEFAULT 'active';
