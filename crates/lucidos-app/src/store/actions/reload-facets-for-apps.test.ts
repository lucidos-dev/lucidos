import { describe, it, expect, vi } from 'vitest';

vi.hoisted(() => {
  const storage = new Map<string, string>();
  (globalThis as any).localStorage = {
    getItem: (k: string) => storage.get(k) ?? null,
    setItem: (k: string, v: string) => storage.set(k, v),
    removeItem: (k: string) => storage.delete(k),
    clear: () => storage.clear(),
    get length() { return storage.size; },
    key: (_i: number) => null,
  };
});

import { fetchFilterFacets } from '../../api/threads';
import { reloadFacetsForApps } from './thread-loading';

vi.mock('../../api/threads', () => ({
  fetchFilterFacets: vi.fn().mockResolvedValue({ triggers: [], repos: [], apps: [] }),
}));

const fetchFacets = fetchFilterFacets as unknown as ReturnType<typeof vi.fn>;

describe('reloadFacetsForApps', () => {
  it('reloads once per id the facets do not label, never in a loop', () => {
    reloadFacetsForApps(['fare-grid']);
    reloadFacetsForApps(['fare-grid']);
    expect(fetchFacets).toHaveBeenCalledTimes(1);

    reloadFacetsForApps(['fare-grid', 'mood-picker']);
    expect(fetchFacets).toHaveBeenCalledTimes(2);
  });
});
