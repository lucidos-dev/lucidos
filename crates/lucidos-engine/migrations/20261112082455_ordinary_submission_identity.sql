-- One immutable acknowledgement per client message identity. The acceptance
-- event has its own id; execution later records under message_event_id.
CREATE UNIQUE INDEX events_ordinary_submission_message_once
ON events ((payload->'data'->>'message_event_id'))
WHERE event_type = 'OrdinarySubmissionAccepted';
