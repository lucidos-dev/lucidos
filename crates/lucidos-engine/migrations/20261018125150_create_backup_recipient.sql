-- The workspace's backup recipient: the age public key every new backup is
-- sealed to, at most one row. Only the user holds the matching backup key,
-- and nothing here or anywhere else on the machine stores it.
--
-- `recipient` seals archives. `pending_recipient` is a freshly minted key
-- pair's public half, waiting for the user to paste its backup key back. It
-- seals nothing until that confirmation promotes it. `verified_at` is when the
-- user last proved they still hold the backup key.

CREATE TABLE IF NOT EXISTS backup_recipient (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    recipient TEXT CHECK (recipient LIKE 'age1%'),
    recipient_set_at TIMESTAMPTZ,
    verified_at TIMESTAMPTZ,
    pending_recipient TEXT CHECK (pending_recipient LIKE 'age1%'),
    CHECK ((recipient IS NULL) = (recipient_set_at IS NULL))
);
