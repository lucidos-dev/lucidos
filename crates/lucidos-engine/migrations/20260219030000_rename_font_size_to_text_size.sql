-- Rename the 'font-size' preference key to 'text-size'
-- Delete font-size rows where a text-size row already exists for the same device
DELETE FROM preferences f
  USING preferences t
  WHERE f.key = 'font-size'
    AND t.key = 'text-size'
    AND COALESCE(f.device_id, '') = COALESCE(t.device_id, '');

-- Rename remaining font-size rows (no conflict)
UPDATE preferences SET key = 'text-size' WHERE key = 'font-size';
