-- Add src_created_at to memory_entries (source event/artifact timestamp).
-- Existing created_at values are source timestamps, so copy them over.
-- Conditional: only runs if memory_entries exists (skip on fresh installs
-- where ensure_schema() hasn't created the table yet).
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'memory_entries') THEN
        IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = 'memory_entries' AND column_name = 'src_created_at') THEN
            ALTER TABLE memory_entries ADD COLUMN src_created_at TIMESTAMPTZ;
            UPDATE memory_entries SET src_created_at = created_at;
            ALTER TABLE memory_entries ALTER COLUMN src_created_at SET NOT NULL;
            CREATE INDEX IF NOT EXISTS memory_entries_src_created_at_idx ON memory_entries (src_created_at DESC);
        END IF;
    END IF;
END $$;
