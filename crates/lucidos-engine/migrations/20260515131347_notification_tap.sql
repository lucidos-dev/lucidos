-- Notification routing: where a push tap should land.
-- 'modal' (default) opens the inbox modal so the user can read the message
-- and choose what to do next. 'open_app' / 'open_thread' are explicit opt-ins
-- that skip the modal and deep-link straight to the linked app/thread.
ALTER TABLE notifications
    ADD COLUMN tap TEXT NOT NULL DEFAULT 'modal';

ALTER TABLE notifications
    ADD CONSTRAINT notifications_tap_valid
    CHECK (tap IN ('modal', 'open_app', 'open_thread'));

ALTER TABLE notifications
    ADD CONSTRAINT notifications_open_app_requires_app_id
    CHECK (tap <> 'open_app' OR app_id IS NOT NULL);

ALTER TABLE notifications
    ADD CONSTRAINT notifications_open_thread_requires_thread_id
    CHECK (tap <> 'open_thread' OR thread_id IS NOT NULL);
