/** Update actions load after paint. The entry reaches their refresh through
 * `releaseCheckHandle.ts` and the deferred background facade. */
import { describe, it, expect } from 'vitest';
import { chainTo, srcPath, staticGraph } from './staticImportGraph';

describe('the entry chunk loads the app update actions after paint', () => {
  const graph = staticGraph(srcPath('main.tsx'));

  it('reaches the release check handle, so the walk itself works', () => {
    expect(graph.has(srcPath('store/releaseCheckHandle.ts'))).toBe(true);
  });

  it('never reaches store/actions/app-update.ts by a static import', () => {
    const path = srcPath('store/actions/app-update.ts');
    expect(graph.has(path), graph.has(path) ? chainTo(graph, path) : '').toBe(false);
  });
});
