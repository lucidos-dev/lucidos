ALTER TABLE thread_summaries ADD COLUMN parent_thread_id UUID;
CREATE INDEX idx_thread_summaries_parent ON thread_summaries(parent_thread_id)
    WHERE parent_thread_id IS NOT NULL;
