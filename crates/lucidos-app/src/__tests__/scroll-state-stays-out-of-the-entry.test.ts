/**
 * The transcript's scroll engine ships in the shell chunk, never the entry.
 * The store reaches it through `components/chat/scrollStateHandle.ts`, which
 * `startShell` fills. One static import of `scrollState` from anything
 * `main.tsx` reaches puts all of it back on first paint, about 22 kB.
 *
 * This walks the static import graph from `main.tsx`, as Rollup does for the
 * entry chunk (`staticImportGraph.ts`).
 */
import { describe, it, expect } from 'vitest';
import { chainTo, srcPath, staticGraph } from './staticImportGraph';

describe('the entry chunk leaves the scroll engine to the shell', () => {
  const graph = staticGraph(srcPath('main.tsx'));
  const scrollState = srcPath('components/chat/scrollState.ts');

  it('reaches the store and the handle, so the walk itself works', () => {
    expect(graph.has(srcPath('store/actions/threads.ts'))).toBe(true);
    expect(graph.has(srcPath('components/chat/scrollStateHandle.ts'))).toBe(true);
  });

  it('never reaches scrollState.ts by a static import', () => {
    expect(graph.has(scrollState), graph.has(scrollState) ? chainTo(graph, scrollState) : '').toBe(false);
  });
});
