-- Exact restoration additions survive PUT acknowledgements and reconnects.
-- Consume clears the journal; the removal event remains the replay authority.
ALTER TABLE thread_summaries ADD COLUMN compose_restorations JSONB NOT NULL DEFAULT '[]'::jsonb
    CHECK (jsonb_typeof(compose_restorations) = 'array');
