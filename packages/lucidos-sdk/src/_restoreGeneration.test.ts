import { describe, expect, it } from 'vitest';
import { RestoreGenerationFence } from './_restoreGeneration';
const first = '11111111-1111-4111-8111-111111111111';
const second = '22222222-2222-4222-8222-222222222222';
describe('direct SDK restore provenance', () => {
  it('never upgrades a request captured before the first hello', () => {
    const fence = new RestoreGenerationFence();
    const pending = fence.capture();
    fence.observe(first);
    expect(pending.header).toBe('none');
    expect(() => pending.assertCurrent()).toThrow(/restored/i);
    expect(() => fence.capture().assertCurrent()).toThrow(/restored/i);
  });
  it('closes a page across reconnect and return to its old generation', () => {
    const fence = new RestoreGenerationFence();
    fence.observe(first);
    const pending = fence.capture();
    fence.observe(second);
    fence.observe(first);
    expect(() => pending.assertCurrent()).toThrow(/restored/i);
    expect(() => fence.capture().assertCurrent()).toThrow(/restored/i);
  });
  it('fails closed on malformed metadata', () => {
    for (const malformed of [undefined, '', 'none', {}, 'not-a-uuid']) {
      const fence = new RestoreGenerationFence();
      fence.observe(malformed);
      expect(() => fence.capture().assertCurrent()).toThrow(/restored/i);
    }
  });
  it('admits matching original and restored generations', () => {
    for (const generation of [null, first]) {
      const fence = new RestoreGenerationFence();
      fence.observe(generation);
      const pending = fence.capture();
      fence.observe(generation);
      expect(() => pending.assertCurrent()).not.toThrow();
      expect(pending.header).toBe(generation ?? 'none');
    }
  });
});
