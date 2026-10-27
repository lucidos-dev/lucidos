-- The keep-alive is now off by default, so its break-even mode no longer
-- answers to 'default'. A stored pick keeps its meaning under the new name.
UPDATE preferences SET value = 'break-even'
  WHERE key = 'cache_keep_alive_mode' AND value = 'default';
