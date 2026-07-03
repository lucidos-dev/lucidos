-- Rename reviewed → hardened on the changes table.
ALTER TABLE changes RENAME COLUMN reviewed TO hardened;

-- Rename the MissingCodeReviewDetected event type to MissingHardeningDetected.
UPDATE events
SET event_type = 'MissingHardeningDetected'
WHERE event_type = 'MissingCodeReviewDetected';
