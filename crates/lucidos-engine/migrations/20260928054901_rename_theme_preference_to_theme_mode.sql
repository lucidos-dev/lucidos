-- The light/dark/system preference is the theme mode. Its key 'theme' becomes
-- 'theme-mode', so 'theme' is free to name the palette (docs/plans/2026-09-27-look-becomes-theme.md).
-- This runs before the migration that renames 'look' to 'theme'.

-- A 'theme-mode' row already on the same device wins over the old one.
DELETE FROM preferences f
  USING preferences t
  WHERE f.key = 'theme'
    AND t.key = 'theme-mode'
    AND COALESCE(f.device_id, '') = COALESCE(t.device_id, '');

UPDATE preferences SET key = 'theme-mode' WHERE key = 'theme';
