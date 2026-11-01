/**
 * The drawer's family graph ships in the shell chunk, never the entry. The
 * store reads the drawer's order through `store/drawerOrderHandle.ts`, which
 * `startShell` provides. One static import of the graph from anything
 * `main.tsx` reaches puts it back on first paint, about 8 kB.
 *
 * This walks the static import graph from `main.tsx`, as Rollup does for the
 * entry chunk (`staticImportGraph.ts`).
 */
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
