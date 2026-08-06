-- Compose epoch: how many times this thread's compose slot has been consumed
-- by a submission (a sent message, a coding-agent session start, a question
-- answer that WAS the draft). It is a submission counter, NOT a write counter:
-- every compose PUT between two submissions carries the same epoch, so the
-- keystroke path never hits the precondition.
--
-- `PUT /api/v1/threads/:id/compose` echoes the epoch the client last saw and
-- the UPDATE matches on it, so a write composed BEFORE a submission can never
-- be applied AFTER it. Without the fence, a draft PUT stalled by a bad
-- connection lands after the message it preceded and rewrites the draft the
-- send had just consumed, so the message shows as sent while the composer
-- still holds a stale revision of it.
--
-- Existing rows start at 0; a client that sends no epoch is accepted unfenced
-- (a cached PWA bundle running against a newer engine).
ALTER TABLE thread_summaries
    ADD COLUMN IF NOT EXISTS compose_epoch BIGINT NOT NULL DEFAULT 0;
