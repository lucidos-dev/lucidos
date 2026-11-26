import { describe, it, expect } from 'vitest';
import { chainTo, srcPath, staticGraph } from './staticImportGraph';

describe('the entry chunk leaves the scroll engine to the shell', () => {
  const graph = staticGraph(srcPath('main.tsx'));
  const engineModules = [
    'scrollState', 'readerInput', 'scrollLedger', 'classifiedScroll', 'scrollOwner',
    'scrollSurface', 'scrollNavigation', 'scrollHold', 'scrollRide', 'scrollLink',
    'scrollLanding', 'scrollObservers',
  ].map((name) => srcPath(`components/chat/${name}.ts`));

  it('reaches the store and the handle, so the walk itself works', () => {
    expect(graph.has(srcPath('store/actions/threads.ts'))).toBe(true);
    expect(graph.has(srcPath('components/chat/scrollStateHandle.ts'))).toBe(true);
  });

  it.each(engineModules)('never reaches %s by a static import', (module) => {
    expect(graph.has(module), graph.has(module) ? chainTo(graph, module) : '').toBe(false);
  });
});
