import { describe, it, expect } from 'vitest';
import STARTUP from '../../../store/startup.ts?raw';

describe('startup', () => {
  it('preloads both plugin sources', () => {
    expect(STARTUP).toMatch(/void loadPluginCatalog\(\);/);
    expect(STARTUP).toMatch(/void loadInstalledPlugins\(\);/);
  });
});
