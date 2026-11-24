import { defineConfig, mergeConfig } from 'vitest/config';
import viteConfig from './vite.config';
import { WarmFirstWaveSequencer } from './vite/warmFirstWaveSequencer';

export default mergeConfig(viteConfig, defineConfig({
  test: { sequence: { sequencer: WarmFirstWaveSequencer } },
}));
