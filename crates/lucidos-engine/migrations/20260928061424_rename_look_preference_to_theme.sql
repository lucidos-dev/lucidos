-- The palette preference is the theme. 'look' becomes 'theme' and
-- 'look-effects' becomes 'theme-effects', and a 'font-family' that followed
-- the look now follows the theme (docs/plans/2026-09-27-look-becomes-theme.md).
-- Runs after the migration that moved the mode from 'theme' to 'theme-mode',
-- so no row under 'theme' can still hold a mode here.

-- A row already under the new key on the same device wins over the old one.
DELETE FROM preferences f
  USING preferences t
  WHERE f.key = 'look'
    AND t.key = 'theme'
    AND COALESCE(f.device_id, '') = COALESCE(t.device_id, '');
UPDATE preferences SET key = 'theme' WHERE key = 'look';

DELETE FROM preferences f
  USING preferences t
  WHERE f.key = 'look-effects'
    AND t.key = 'theme-effects'
    AND COALESCE(f.device_id, '') = COALESCE(t.device_id, '');
UPDATE preferences SET key = 'theme-effects' WHERE key = 'look-effects';

UPDATE preferences SET value = 'theme' WHERE key = 'font-family' AND value = 'look';
