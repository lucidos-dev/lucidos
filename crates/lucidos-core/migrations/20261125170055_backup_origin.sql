CREATE TABLE backup_origin (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    workspace_path text NOT NULL,
    origin text NOT NULL CHECK (origin ~ '^[0-9a-f]{12}$')
);
