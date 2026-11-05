import { beforeEach, expect, it, vi } from 'vitest';
beforeEach(() => vi.resetModules());
it('retains untagged and old sources after restore, including late writes by an old page', async () => {
  const source = await import('./restoreSourceGeneration');
  const lateWrite = { sourceGeneration: source.sourceGeneration() };
  source.observeSourceGeneration('restored');
  expect(source.isHistoricalRestoreSource(lateWrite)).toBe(true);
  expect(source.isHistoricalRestoreSource({})).toBe(true);
  expect(source.isHistoricalRestoreSource({ sourceGeneration: null })).toBe(true);
  expect(source.isHistoricalRestoreSource({ sourceGeneration: 'previous' })).toBe(true);
});
it('allows ordinary reload recovery only within the same observed generation', async () => {
  const source = await import('./restoreSourceGeneration');
  source.observeSourceGeneration('restored');
  const current = { sourceGeneration: source.sourceGeneration() };
  expect(source.isHistoricalRestoreSource(current)).toBe(false);
  source.observeSourceGeneration('next');
  expect(source.isHistoricalRestoreSource(current)).toBe(true);
});
it('binds an early request to the first admission only when it is proven since page load', async () => {
  const source = await import('./restoreSourceGeneration');
  const early = source.bindSourceGeneration();
  source.observeSourceGeneration('restored', true);
  expect(early()).toBe('restored');
  source.observeSourceGeneration('next');
  expect(early()).toBe('restored');
  expect(source.bindSourceGeneration()()).toBe('next');
});
it('binds an early request to no generation when the page-load baseline was never read', async () => {
  const source = await import('./restoreSourceGeneration');
  const early = source.bindSourceGeneration();
  source.observeSourceGeneration('restored', false);
  expect(early()).toBe(null);
  expect(source.bindSourceGeneration()()).toBe('restored');
});
it('preserves legacy reload behavior before the first restore', async () => {
  const source = await import('./restoreSourceGeneration');
  expect(source.isHistoricalRestoreSource({})).toBe(false);
  source.observeSourceGeneration(null);
  expect(source.isHistoricalRestoreSource({})).toBe(false);
  expect(source.sourceGeneration()).toBe(null);
});
