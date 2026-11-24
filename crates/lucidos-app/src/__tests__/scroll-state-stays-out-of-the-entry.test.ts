import { describe, it, expect } from 'vitest';
import { chainTo, srcPath, staticGraph } from './staticImportGraph';

describe('the entry chunk leaves the scroll engine to the shell', () => {
  const graph = staticGraph(srcPath('main.tsx'));
  const engineModules = ['scrollState.ts', 'readerInput.ts', 'scrollLedger.ts', 'classifiedScroll.ts'].map((name) => srcPath(`components/chat/${name}`));

  it('reaches the store and the handle, so the walk itself works', () => {
    expect(graph.has(srcPath('store/actions/threads.ts'))).toBe(true);
    expect(graph.has(srcPath('components/chat/scrollStateHandle.ts'))).toBe(true);
  });

  it.each(engineModules)('never reaches %s by a static import', (module) => {
    expect(graph.has(module), graph.has(module) ? chainTo(graph, module) : '').toBe(false);
  });
});
