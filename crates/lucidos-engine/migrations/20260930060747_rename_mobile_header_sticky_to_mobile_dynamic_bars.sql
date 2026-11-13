-- 'mobile_header_sticky' becomes 'mobile_dynamic_bars', with the polarity
-- flipped: a pinned header ('true') is dynamic bars off, and the reverse
-- (docs/plans/2026-09-30-mobile-dynamic-bars.md). A value that is neither
-- 'true' nor 'false' read as pinned before, so it stays pinned.

-- A row already under the new key on the same device wins over the old one.
DELETE FROM preferences f
  USING preferences t
  WHERE f.key = 'mobile_header_sticky'
    AND t.key = 'mobile_dynamic_bars'
    AND COALESCE(f.device_id, '') = COALESCE(t.device_id, '');

UPDATE preferences
  SET key = 'mobile_dynamic_bars',
      value = CASE WHEN value = 'false' THEN 'true' ELSE 'false' END
  WHERE key = 'mobile_header_sticky';
