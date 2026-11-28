import { describe, it, expect } from 'vitest';
import { chainTo, srcPath, staticGraph } from './staticImportGraph';

describe('the entry chunk leaves the family graph to the shell', () => {
  const graph = staticGraph(srcPath('main.tsx'));

  it('reaches the drawer order handle, so the walk itself works', () => {
    expect(graph.has(srcPath('store/drawerOrderHandle.ts'))).toBe(true);
  });

  for (const module of ['components/drawer/family-graph.ts', 'components/drawer/drawerOrder.ts']) {
    it(`never reaches ${module} by a static import`, () => {
      const path = srcPath(module);
      expect(graph.has(path), graph.has(path) ? chainTo(graph, path) : '').toBe(false);
    });
  }
});
