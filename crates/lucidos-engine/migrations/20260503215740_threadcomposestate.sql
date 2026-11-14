-- Compose state on the thread aggregate. Replaces the parallel `drafts` table.
-- A draft IS a thread in `composing` state; once `MessageReceived` fires the
-- thread transitions to `active`. `ThreadDiscarded` is terminal — the
-- state-machine guard at the API boundary rejects all further compose PUTs /
-- message POSTs, which is the "make impossible states impossible" lever
-- replacing the old LWW + tombstone machinery.

ALTER TABLE thread_summaries
  ADD COLUMN IF NOT EXISTS state           TEXT NOT NULL DEFAULT 'composing',
  ADD COLUMN IF NOT EXISTS compose_text    TEXT NOT NULL DEFAULT '',
  ADD COLUMN IF NOT EXISTS compose_images  JSONB NOT NULL DEFAULT '[]'::jsonb,
  ADD COLUMN IF NOT EXISTS compose_mode    TEXT;

-- Existing rows: any thread that ever received a message is `active`.
-- Threads with zero messages don't currently exist (the projection only
-- inserts on MessageReceived / SessionStarted / TriggerStarted), so this
-- update covers everything in the table.
UPDATE thread_summaries SET state = 'active' WHERE message_count > 0;

-- A thread that's been archived (archive_state = 'archived') stays 'active'
-- in the compose state machine — `archive_state` is an orthogonal axis.
-- The state-machine guard treats Active and Composing identically for
-- followup compose updates.

CREATE INDEX IF NOT EXISTS thread_summaries_state_idx ON thread_summaries(state);
