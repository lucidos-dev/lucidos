-- Repair threads the `SessionStarted | ContinuationStarted` projection arm
-- wrongly relabeled as coding-agent threads.
--
-- That arm hardcoded `is_coding_agent = TRUE`, but `ContinuationStarted` is a
-- channel-agnostic resume boundary: the chat and trigger Continue paths emit it
-- too (`chat/rerun.rs`'s `emit_resume_anchor`, reached from
-- `POST /api/v1/threads/:thread_id/continue`). So one Continue click on an
-- ordinary chat thread permanently flipped the flag — and `continue_thread`
-- dispatches on it, so the NEXT click took the coding-agent branch and never
-- reached `continue_chat` at all. The projection now gates the write on the
-- ClaudeCode channel; this repairs the rows written before that.
--
-- Deliberately conservative: the flag is cleared only for threads that are
-- PROVABLY not coding-agent threads — no `SessionStarted` and no `CodingAgent%`
-- event ever landed on them. Clearing on `source` alone would be wrong: a real
-- coding-agent thread can carry `source = 'chat'` from unrelated legacy drift
-- (one such row exists in a live workspace, with 3 `SessionStarted` and ~1900
-- `CodingAgent*` events), and clearing ITS flag would drop it out of the
-- `settle_orphaned_running_coding_agent_threads` boot sweep. Under-repairing
-- costs a stale flag on an odd row; over-repairing breaks recovery.
--
-- `source <> 'claude_code'` rather than `= 'chat'` so the trigger variant of the
-- same bug is covered. `LIKE 'CodingAgent%'` is safe as a "this thread really
-- ran a coding agent" test: `CodingAgentThreadSpawned` is emitted on the
-- coding-agent thread itself (`claude_code/spawn.rs` binds `cc_thread_id`), not
-- on the spawning parent, so a chat parent never carries one.
UPDATE thread_summaries ts
SET is_coding_agent = FALSE
WHERE ts.is_coding_agent
  AND ts.source <> 'claude_code'
  AND NOT EXISTS (
      SELECT 1 FROM events e
      WHERE e.aggregate_id = ts.thread_id::text
        AND e.event_type = 'SessionStarted'
  )
  AND NOT EXISTS (
      SELECT 1 FROM events e
      WHERE e.aggregate_id = ts.thread_id::text
        AND e.event_type LIKE 'CodingAgent%'
  );
