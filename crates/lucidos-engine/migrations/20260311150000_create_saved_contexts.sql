-- Create saved_contexts table for storing saved context snapshots
CREATE TABLE saved_contexts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    label TEXT NOT NULL,
    model TEXT,
    total_chars INTEGER NOT NULL,
    sections JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Index on created_at for chronological queries
CREATE INDEX idx_saved_contexts_created_at ON saved_contexts(created_at DESC);
