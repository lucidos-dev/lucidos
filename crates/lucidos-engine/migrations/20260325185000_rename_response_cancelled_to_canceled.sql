-- Rename ResponseCancelled → ResponseCanceled (American English, one L)
-- to match the codebase convention used everywhere else.
UPDATE events SET event_type = 'ResponseCanceled' WHERE event_type = 'ResponseCancelled';
