-- `thread_input_chains`: the last input each page admitted to each thread
-- (ADR 0449).
--
-- A page (the *input chain*) numbers its inputs to a thread. Admission, under
-- the thread append lock, admits an input only past `last_seq`, or as a repeat
-- of the tail itself (same `last_seq`, same `last_event_id`). So a send the
-- client gave up on can never land after a later one from the same page.
--
-- No foreign key to `thread_summaries`: a thread's first input is admitted
-- before its first event creates that row. Deleting a thread deletes its rows.

CREATE TABLE thread_input_chains (
    thread_id UUID NOT NULL,
    input_chain UUID NOT NULL,
    last_seq BIGINT NOT NULL,
    last_event_id UUID NOT NULL,
    PRIMARY KEY (thread_id, input_chain)
);
