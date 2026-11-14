ALTER TABLE thread_summaries ADD COLUMN IF NOT EXISTS active_children_count INTEGER NOT NULL DEFAULT 0;
