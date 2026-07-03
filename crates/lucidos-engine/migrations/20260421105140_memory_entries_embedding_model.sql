-- Skip on fresh installs: PgVectorIndex::init_schema() creates the column
-- directly in the CREATE TABLE, so the migration only needs to backfill
-- pre-existing tables.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'memory_entries') THEN
        IF NOT EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_name = 'memory_entries' AND column_name = 'embedding_model'
        ) THEN
            ALTER TABLE memory_entries
                ADD COLUMN embedding_model TEXT NOT NULL DEFAULT 'bge-small-en-v1.5';

            -- Drop the default so future inserts MUST specify the model explicitly.
            ALTER TABLE memory_entries
                ALTER COLUMN embedding_model DROP DEFAULT;

            CREATE INDEX IF NOT EXISTS memory_entries_embedding_model_idx
                ON memory_entries (embedding_model);
        END IF;
    END IF;
END $$;
