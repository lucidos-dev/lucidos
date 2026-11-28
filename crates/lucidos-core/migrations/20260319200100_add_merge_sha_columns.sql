-- Store the commit range applied to main so revert can reliably find what to undo.
-- pre_merge_sha: HEAD of main before the merge/ff
-- post_merge_sha: HEAD of main after the merge/ff
ALTER TABLE changes ADD COLUMN IF NOT EXISTS pre_merge_sha TEXT;
ALTER TABLE changes ADD COLUMN IF NOT EXISTS post_merge_sha TEXT;
