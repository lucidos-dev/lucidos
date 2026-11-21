/**
 * The Plugins panel's data loads at startup, not when the panel mounts.
 *
 * StoreTab holds its skeleton until the catalog AND the installed list have
 * settled. Loaded only from its mount effect, a first open waited for the
 * lazy chunk, the mount, then both round trips. On an iOS PWA, which is
 * evicted constantly, that was nearly every open.
 *
 * Source-scan, mirroring `store-tab-no-poll.test.ts`: running `startClient` in
 * jsdom would pull in the whole startup graph for one call site.
 */
import { describe, it, expect } from 'vitest';
import STARTUP from '../../../store/startup.ts?raw';

describe('startup', () => {
  it('preloads both plugin sources', () => {
    expect(STARTUP).toMatch(/void loadPluginCatalog\(\);/);
    expect(STARTUP).toMatch(/void loadInstalledPlugins\(\);/);
  });
});
