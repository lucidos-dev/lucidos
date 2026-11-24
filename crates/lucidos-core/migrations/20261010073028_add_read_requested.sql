-- Add `read_requested` to `thread_summaries` (ADR 0409).
--
-- TRUE while the thread's agent has asked the user to read its latest reply
-- and the user has not seen it yet. `ThreadReadRequested` sets it.
-- `ThreadReplySeen`, a human message and an archive clear it.
--
-- It feeds neither `is_attention_needing` nor `is_blocking`: the drawer's
-- Review group reads it, and a read request is never something a thread is
-- blocked on.
--
-- No backfill. No thread could ask before this column existed.

ALTER TABLE thread_summaries
  ADD COLUMN read_requested BOOLEAN NOT NULL DEFAULT FALSE;
