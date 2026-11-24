-- A thread waiting on the user is never archived (ADR 0259). An older engine
-- archived an unattended trigger run the moment it asked a question, with no
-- ThreadArchived event behind it. Replaying those events through the fixed
-- contract yields 'inbox', so this makes the projection agree with its events.
--
-- No count needs rebuilding: is_blocking and is_attention_needing both count a
-- waiting thread whatever its archive state.

UPDATE thread_summaries
SET archive_state = 'inbox'
WHERE status = 'waiting_for_user_answer' AND archive_state <> 'inbox';
