/**
 * Vitest reads this file in place of `vite.config.ts`, and `vite build` never
 * reads it. So `vitest/node` stays out of every Vite build's bundled config.
 */
import { defineConfig, mergeConfig } from 'vitest/config';
import viteConfig from './vite.config';
import { WarmFirstWaveSequencer } from './vite/warmFirstWaveSequencer';

export default mergeConfig(viteConfig, defineConfig({
  // Warms the files that start together, so no timed test pays a cold transform.
  test: { sequence: { sequencer: WarmFirstWaveSequencer } },
}));
