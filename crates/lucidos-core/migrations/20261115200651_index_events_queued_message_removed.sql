-- The consumption claim and the Stop-hold reader ask whether the user took a
-- message back. Both run on hot paths, the claim under the thread append lock.
-- Removals are rare, so this partial index stays tiny and turns each question
-- into one probe instead of a walk over the thread's events.
CREATE INDEX IF NOT EXISTS idx_events_queued_message_removed
  ON events (thread_id, (payload->>'removed_message_id'))
  WHERE event_type = 'QueuedMessageRemoved';
