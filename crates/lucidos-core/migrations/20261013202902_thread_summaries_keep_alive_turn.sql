-- True while a cache keep-alive run holds the thread. The run sets it once it
-- has claimed the thread and clears it at the turn's idle; a card and boot
-- clear it too. The projection skips that turn's output while it is set.
ALTER TABLE thread_summaries ADD COLUMN keep_alive_turn BOOLEAN NOT NULL DEFAULT FALSE;
