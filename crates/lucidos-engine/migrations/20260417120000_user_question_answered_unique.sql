-- Enforce single-answer-per-question for the CC AskUserQuestion flow.
-- Without this, two concurrent POSTs to /api/claude-code/answer-question can
-- both pass the application-level idempotency check (read-then-emit, not
-- transactional) and both emit UserQuestionAnswered for the same tool_use_id.
-- The partial unique index makes the second emit fail at the DB level so the
-- API can return 409 instead of double-resuming CC.
CREATE UNIQUE INDEX IF NOT EXISTS events_user_question_answered_unique
    ON events ((thread_id), ((payload->>'tool_use_id')))
    WHERE event_type = 'UserQuestionAnswered';
