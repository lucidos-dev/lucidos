-- The keep-alive is now either off or on. Both modes that kept a cache warm
-- read as on, so no user's keep-alive turns off after the upgrade.
UPDATE preferences SET value = 'on'
  WHERE key = 'cache_keep_alive_mode' AND value IN ('break-even', 'workspace-data');
