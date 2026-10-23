/**
 * The plugins panel refreshes from a frame, never from a clock.
 *
 * It used to install a five-minute `setInterval` on mount. Every tick ran
 * `refreshPluginCatalog`, and that scan git-clones every registered
 * marketplace: seconds per repo, forever, for a panel somebody left open.
 * `.claude/rules/frontend.md` bans a poll outright, and bans the three things a
 * removed poll is usually replaced with.
 *
 * The mount fetch plus the SSE arm already keep the catalog fresh, so the
 * interval bought nothing. Both halves are asserted here: the ban on this
 * surface, and the subscription that makes the ban safe.
 *
 * Source-scan, mirroring `store/actions/sse-event-coverage.test.ts`, which holds
 * the same line for the subscriber modules. A rendered test would have to wait
 * out five real minutes to see the tick. The sources arrive as `?raw` strings so
 * this file needs no Node imports.
 */
import { describe, it, expect } from 'vitest';
import STORE_TAB from '../StoreTab.tsx?raw';
import DISPATCHER from '../../../store/actions/entityReferences.ts?raw';

/** The four shapes the rule names. Each makes one symptom go away and leaves
 *  the rule unenforced, so the next surface repeats the bug. */
const BANNED: Array<[string, RegExp]> = [
  ['an interval', /setInterval\s*\(/],
  ['a visibility listener', /'visibilitychange'/],
  ['a focus listener', /'focus'\s*,/],
  ['a refresh constant for a timer', /CATALOG_REFRESH_MS/],
];

describe('StoreTab', () => {
  it.each(BANNED)('answers a catalog change with no %s', (what, pattern) => {
    expect(pattern.test(STORE_TAB), `StoreTab.tsx still carries ${what}: ${pattern}`).toBe(false);
  });

  it('still refreshes both sources when the panel opens', () => {
    expect(STORE_TAB).toMatch(/void refreshPluginCatalog\(\);/);
    expect(STORE_TAB).toMatch(/void loadInstalledPlugins\(\);/);
  });
});

describe('the marketplace SSE arm the panel relies on', () => {
  it('reloads the catalog on both marketplace frames', () => {
    expect(DISPATCHER).toMatch(/case 'PluginMarketplaceRegistered':/);
    expect(DISPATCHER).toMatch(/case 'PluginMarketplaceRemoved':/);
    expect(DISPATCHER).toMatch(/refreshPluginCatalogAfterMutation\(\)/);
  });
});
