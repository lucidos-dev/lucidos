-- Add `apply_in_flight` to `thread_summaries` (ADR 0437).
--
-- TRUE from an Apply Now's `ApplyNowStarted` to its `ApplyNowEnded`. Every
-- client reads "is this thread applying" from here, so none of them guesses it
-- from other events or a timer. Engine startup ends an apply a stop cut off.
--
-- No backfill. No apply could be in flight before this column existed.

ALTER TABLE thread_summaries
  ADD COLUMN apply_in_flight BOOLEAN NOT NULL DEFAULT FALSE;
