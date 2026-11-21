/**
 * Vitest's own file order, with the first wave warmed (`warmFirstWave.ts`).
 * Kept apart because `vitest/node` cannot load inside a test worker.
 */
import { BaseSequencer, type TestSpecification } from 'vitest/node';
import { warmFirstWave } from './warmFirstWave';

export class WarmFirstWaveSequencer extends BaseSequencer {
  async sort(specs: TestSpecification[]): Promise<TestSpecification[]> {
    const sorted = await super.sort(specs);
    const { maxWorkers, watch } = this.ctx.config;
    await warmFirstWave(sorted, { maxWorkers, watch });
    return sorted;
  }
}
