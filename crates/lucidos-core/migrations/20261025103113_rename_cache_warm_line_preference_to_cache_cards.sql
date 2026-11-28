-- The cache-warm line switch now hides every prompt-cache card, so
-- 'cache_warm_line' becomes 'cache_cards'
-- (docs/plans/2026-10-25-one-cache-cards-switch.md).

-- A row already under the new key on the same device wins over the old one.
DELETE FROM preferences f
  USING preferences t
  WHERE f.key = 'cache_warm_line'
    AND t.key = 'cache_cards'
    AND COALESCE(f.device_id, '') = COALESCE(t.device_id, '');
UPDATE preferences SET key = 'cache_cards' WHERE key = 'cache_warm_line';
