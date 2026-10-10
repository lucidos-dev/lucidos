-- Notifications now carry the thread that produced them so the inbox modal
-- can deep-link back. Engine already computed this thread for push deep-links;
-- this column lets the in-app surface read the same value.
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS thread_id UUID;
