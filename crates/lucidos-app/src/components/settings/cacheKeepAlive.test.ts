import { describe, it, expect } from 'vitest';
import {
  CACHE_KEEP_ALIVE_OPTIONS,
  isCacheKeepAliveMode,
} from './CacheKeepAliveSection';
import { CACHE_KEEP_ALIVE_MODES } from '../../store/actions/preferences';
import { PREFERENCE_CATALOG } from '@lucidos/preference-catalog';

describe('cache keep-alive mode', () => {
  it('puts the default first', () => {
    expect(CACHE_KEEP_ALIVE_OPTIONS[0].value)
      .toBe(PREFERENCE_CATALOG.cache_keep_alive_mode.fallback);
  });

  it('offers every catalog value once, with a label and a trade-off line', () => {
    expect(CACHE_KEEP_ALIVE_OPTIONS.map((o) => o.value).sort())
      .toEqual([...CACHE_KEEP_ALIVE_MODES].sort());
    for (const option of CACHE_KEEP_ALIVE_OPTIONS) {
      expect(option.label.length).toBeGreaterThan(0);
      expect(option.description?.length ?? 0).toBeGreaterThan(0);
    }
  });

  it('refuses a value outside the accepted set', () => {
    for (const mode of CACHE_KEEP_ALIVE_MODES) expect(isCacheKeepAliveMode(mode)).toBe(true);
    for (const rejected of ['', 'workspace_data', 'on', 'Default']) {
      expect(isCacheKeepAliveMode(rejected)).toBe(false);
    }
  });
});
